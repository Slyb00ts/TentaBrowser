//! The decoded-image store: keyed, byte-bounded, least-recently-used.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use tenta_core::Budgets;

use super::limits::DecodeParams;
use super::{DecodedImage, ImageId};
use crate::ImageKey;

const MIB: usize = 1024 * 1024;

/// Bytes charged for an entry beyond its pixels and URL: the maps, the key and
/// the image header. Without it a flood of 1x1 images would cost almost nothing
/// while holding real memory.
const ENTRY_OVERHEAD: usize = 256;

/// Pinned images may hold the store above its capacity, but never above this
/// multiple of it.
const HARD_CEILING_FACTOR: usize = 2;

/// What identifies a decoded image: the same bytes decoded for a different
/// target size are different pixels.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct StoreKey {
    /// The resolved image URL.
    pub url: ImageKey,
    /// The decode request.
    pub params: DecodeParams,
}

/// Why an image could not be stored.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum StoreError {
    /// Storing the image would put the pinned images above the hard ceiling.
    Full {
        /// Bytes the pinned images and the new image would occupy.
        needed: usize,
        /// The hard ceiling, twice the capacity.
        limit: usize,
    },
    /// Every image id has been handed out.
    IdsExhausted,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full { needed, limit } => write!(
                f,
                "the image store would hold {needed} pinned bytes, above its {limit} byte ceiling"
            ),
            Self::IdsExhausted => f.write_str("the image store has no ids left"),
        }
    }
}

impl std::error::Error for StoreError {}

/// What a lookup tells layout about a stored image.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct ImageInfo {
    /// Handle to draw the image with.
    pub id: ImageId,
    /// Intrinsic width in image pixels, before any decode-time shrink.
    pub width: u32,
    /// Intrinsic height in image pixels, before any decode-time shrink.
    pub height: u32,
    /// Image pixels per CSS pixel (the `2x` of a `srcset` candidate).
    pub density: f32,
}

impl ImageInfo {
    /// The natural size in CSS pixels.
    pub fn css_size(&self) -> (f32, f32) {
        (
            self.width as f32 / self.density,
            self.height as f32 / self.density,
        )
    }
}

#[derive(Clone, Debug)]
struct Entry {
    key: StoreKey,
    image: DecodedImage,
    density: f32,
    last_used: u64,
    /// Bytes charged to the store for this entry.
    cost: usize,
}

impl Entry {
    fn info(&self, id: ImageId) -> ImageInfo {
        ImageInfo {
            id,
            width: self.image.source_width,
            height: self.image.source_height,
            density: self.density,
        }
    }
}

/// Decoded images by id, bounded in bytes.
///
/// Each image is charged its pixels, its URL and a fixed per-entry overhead.
/// When an insert pushes the total over the capacity, the least recently used
/// images are dropped, except those pinned by [`ImageStore::set_pinned`]:
/// ids that a live display list refers to must keep resolving, so pinned images
/// may hold the store above its capacity until the list is replaced, but an
/// insert that would take the pinned images past twice the capacity is refused.
/// Ids are never reused. The recency clock ticks on insert, lookup and pinning;
/// a plain [`ImageStore::get`] takes `&self` and does not count as use.
#[derive(Clone, Debug)]
pub struct ImageStore {
    entries: HashMap<ImageId, Entry>,
    by_key: HashMap<StoreKey, ImageId>,
    pinned: HashSet<ImageId>,
    /// Unpinned images by last use, oldest first: eviction takes the front.
    recency: BTreeMap<u64, ImageId>,
    next_id: u64,
    clock: u64,
    bytes: usize,
    pinned_bytes: usize,
    capacity: usize,
}

impl Default for ImageStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageStore {
    /// A store with the platform's per-context image cache size.
    pub fn new() -> Self {
        Self::from_budgets(&Budgets::platform_default())
    }

    /// A store capped by the context memory budget.
    pub fn from_budgets(budgets: &Budgets) -> Self {
        Self::with_capacity(budgets.context_memory_mb as usize * MIB)
    }

    /// A store holding at most `capacity` charged bytes when nothing is pinned.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            by_key: HashMap::new(),
            pinned: HashSet::new(),
            recency: BTreeMap::new(),
            next_id: 0,
            clock: 0,
            bytes: 0,
            pinned_bytes: 0,
            capacity,
        }
    }

    /// Stores a decoded image and returns its handle. A key that is already
    /// present keeps its existing image and id.
    ///
    /// Fails, leaving the store unchanged, when the pinned images plus this one
    /// would exceed twice the capacity, or when no ids are left.
    pub fn insert(
        &mut self,
        key: StoreKey,
        image: DecodedImage,
        density: f32,
    ) -> Result<ImageId, StoreError> {
        if let Some(&id) = self.by_key.get(&key) {
            self.touch(id);
            return Ok(id);
        }
        let cost = image
            .rgba
            .len()
            .saturating_add(key.url.0.len())
            .saturating_add(ENTRY_OVERHEAD);
        let limit = self.capacity.saturating_mul(HARD_CEILING_FACTOR);
        let needed = self.pinned_bytes.saturating_add(cost);
        if needed > limit {
            return Err(StoreError::Full { needed, limit });
        }
        let id = u32::try_from(self.next_id)
            .map(ImageId)
            .map_err(|_| StoreError::IdsExhausted)?;
        self.next_id += 1;

        self.bytes += cost;
        self.by_key.insert(key.clone(), id);
        let density = if density.is_finite() && density > 0.0 {
            density
        } else {
            1.0
        };
        self.clock += 1;
        self.recency.insert(self.clock, id);
        self.entries.insert(
            id,
            Entry {
                key,
                image,
                density,
                last_used: self.clock,
                cost,
            },
        );
        self.evict_to(self.capacity, Some(id));
        Ok(id)
    }

    /// Finds a stored image by key and marks it recently used.
    pub fn lookup(&mut self, key: &StoreKey) -> Option<ImageInfo> {
        let id = *self.by_key.get(key)?;
        self.touch(id);
        self.entries.get(&id).map(|entry| entry.info(id))
    }

    /// The pixels behind a handle; `None` once evicted.
    pub fn get(&self, id: ImageId) -> Option<&DecodedImage> {
        self.entries.get(&id).map(|entry| &entry.image)
    }

    /// Size and density behind a handle; `None` once evicted.
    pub fn info(&self, id: ImageId) -> Option<ImageInfo> {
        self.entries.get(&id).map(|entry| entry.info(id))
    }

    /// Replaces the set of images that must stay resident, normally the ids in
    /// the display list being shown. Images that were pinned and no longer are
    /// become evictable, and the store shrinks back to its capacity.
    pub fn set_pinned(&mut self, ids: impl IntoIterator<Item = ImageId>) {
        let pinned: HashSet<ImageId> = ids
            .into_iter()
            .filter(|id| self.entries.contains_key(id))
            .collect();
        for id in self.pinned.difference(&pinned) {
            if let Some(entry) = self.entries.get(id) {
                self.recency.insert(entry.last_used, *id);
            }
        }
        for id in &pinned {
            if let Some(entry) = self.entries.get_mut(id) {
                if !self.pinned.contains(id) {
                    self.recency.remove(&entry.last_used);
                }
                self.clock += 1;
                entry.last_used = self.clock;
            }
        }
        self.pinned_bytes = pinned.iter().map(|id| self.entries[id].cost).sum();
        self.pinned = pinned;
        self.evict_to(self.capacity, None);
    }

    /// Drops unpinned images, least recently used first, until at most
    /// `max_bytes` remain or only pinned images are left.
    pub fn trim(&mut self, max_bytes: usize) {
        self.evict_to(max_bytes, None);
    }

    /// Number of stored images.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when the store holds no image.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bytes currently charged: pixels, URLs and the per-entry overhead.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    fn touch(&mut self, id: ImageId) {
        self.clock += 1;
        let Some(entry) = self.entries.get_mut(&id) else {
            return;
        };
        if !self.pinned.contains(&id) {
            self.recency.remove(&entry.last_used);
            self.recency.insert(self.clock, id);
        }
        entry.last_used = self.clock;
    }

    /// Evicts from the old end of the recency order. `keep` is the image just
    /// inserted, which is the newest, so it is only ever reached when nothing
    /// else evictable is left.
    fn evict_to(&mut self, target: usize, keep: Option<ImageId>) {
        while self.bytes > target {
            let Some((&stamp, &id)) = self.recency.first_key_value() else {
                return;
            };
            if Some(id) == keep {
                return;
            }
            self.recency.remove(&stamp);
            if let Some(entry) = self.entries.remove(&id) {
                self.bytes -= entry.cost;
                self.by_key.remove(&entry.key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn key(url: &str) -> StoreKey {
        StoreKey {
            url: ImageKey(Arc::from(url)),
            params: DecodeParams::default(),
        }
    }

    /// A `side x side` image: `side * side * 4` bytes of pixels.
    fn image(side: u32) -> DecodedImage {
        DecodedImage {
            width: side,
            height: side,
            rgba: vec![0; (side * side * 4) as usize].into(),
            source_width: side * 2,
            source_height: side * 2,
        }
    }

    /// What the store charges for a `side x side` image stored under `url`.
    fn cost(side: u32, url: &str) -> usize {
        (side * side * 4) as usize + url.len() + ENTRY_OVERHEAD
    }

    fn insert(store: &mut ImageStore, url: &str, side: u32) -> ImageId {
        store.insert(key(url), image(side), 1.0).expect("stored")
    }

    #[test]
    fn ids_are_dense_and_resolve() {
        let mut store = ImageStore::with_capacity(1 << 20);
        assert!(store.is_empty());
        let a = insert(&mut store, "a", 1);
        let b = insert(&mut store, "b", 1);
        assert_eq!((a, b), (ImageId(0), ImageId(1)));
        assert_eq!(store.get(b).map(|i| i.width), Some(1));
        assert_eq!(store.get(ImageId(2)), None);
        assert_eq!(
            (store.len(), store.bytes()),
            (2, cost(1, "a") + cost(1, "b"))
        );
    }

    #[test]
    fn every_entry_pays_a_fixed_overhead_and_its_url() {
        let mut store = ImageStore::with_capacity(1 << 20);
        let url = "https://example.test/a-rather-long-image-url.png";
        insert(&mut store, url, 1);
        assert_eq!(store.bytes(), 4 + url.len() + ENTRY_OVERHEAD);
        assert!(store.bytes() >= 256 + url.len());
    }

    #[test]
    fn tiny_images_cannot_flood_the_store() {
        // Without the overhead 1x1 images would be 4 bytes each.
        let mut store = ImageStore::with_capacity(10 * cost(1, "00"));
        for i in 0..100 {
            insert(&mut store, &format!("{i:02}"), 1);
        }
        assert!(store.len() <= 10, "{} entries kept", store.len());
        assert!(store.bytes() <= 10 * cost(1, "00"));
    }

    #[test]
    fn lookup_reports_intrinsic_size_and_density() {
        let mut store = ImageStore::with_capacity(1 << 20);
        let id = store.insert(key("a"), image(4), 2.0).expect("stored");
        let info = store.lookup(&key("a")).expect("stored");
        assert_eq!(info.id, id);
        // The pixels were shrunk to 4x4 from an 8x8 source.
        assert_eq!((info.width, info.height), (8, 8));
        assert_eq!(info.css_size(), (4.0, 4.0));
        assert_eq!(store.get(id).map(|i| i.width), Some(4));
        assert_eq!(store.lookup(&key("missing")), None);
    }

    #[test]
    fn decode_params_are_part_of_the_key() {
        let mut store = ImageStore::with_capacity(1 << 20);
        let small = StoreKey {
            params: DecodeParams {
                max_size: Some((8, 8)),
            },
            ..key("a")
        };
        let a = insert(&mut store, "a", 4);
        let b = store.insert(small.clone(), image(2), 1.0).expect("stored");
        assert_ne!(a, b);
        assert_eq!(store.lookup(&small).map(|i| i.id), Some(b));
    }

    #[test]
    fn inserting_a_present_key_keeps_the_first_image() {
        let mut store = ImageStore::with_capacity(1 << 20);
        let a = insert(&mut store, "a", 1);
        assert_eq!(insert(&mut store, "a", 2), a);
        assert_eq!((store.len(), store.bytes()), (1, cost(1, "a")));
    }

    #[test]
    fn the_least_recently_used_image_goes_first() {
        // Two images fit; three do not.
        let mut store = ImageStore::with_capacity(2 * cost(2, "a") + 10);
        let a = insert(&mut store, "a", 2);
        let b = insert(&mut store, "b", 2);
        store.lookup(&key("a"));
        let c = insert(&mut store, "c", 2);
        assert!(store.get(a).is_some() && store.get(c).is_some());
        assert!(store.get(b).is_none());
        assert_eq!(store.lookup(&key("b")), None);
        assert!(store.bytes() <= 2 * cost(2, "a") + 10);
    }

    #[test]
    fn pinned_images_survive_pressure_and_free_up_afterwards() {
        let capacity = cost(2, "a") + 10;
        let mut store = ImageStore::with_capacity(capacity);
        let a = insert(&mut store, "a", 2);
        store.set_pinned([a]);
        let b = insert(&mut store, "b", 2);
        let c = insert(&mut store, "c", 2);
        assert!(store.get(a).is_some(), "pinned");
        assert!(store.get(b).is_none(), "evicted for c");
        assert!(
            store.get(c).is_some(),
            "the new image is never its own victim"
        );
        store.set_pinned([]);
        assert!(store.bytes() <= capacity);
        assert!(store.get(a).is_none(), "oldest, no longer pinned");
    }

    #[test]
    fn pinned_images_cannot_exceed_twice_the_capacity() {
        let one = cost(2, "a");
        let mut store = ImageStore::with_capacity(one);
        let a = insert(&mut store, "a", 2);
        store.set_pinned([a]);
        let b = insert(&mut store, "b", 2);
        store.set_pinned([a, b]);
        let before = (store.len(), store.bytes());
        // Two pinned entries fill the ceiling of 2 * capacity; a third cannot fit.
        let refused = store.insert(key("c"), image(2), 1.0);
        assert!(
            matches!(refused, Err(StoreError::Full { .. })),
            "{refused:?}"
        );
        assert_eq!((store.len(), store.bytes()), (before.0, before.1));
        assert!(store.lookup(&key("c")).is_none());
        assert!(store.get(a).is_some() && store.get(b).is_some());
        // Unpinning makes room again.
        store.set_pinned([]);
        assert!(store.insert(key("c"), image(2), 1.0).is_ok());
    }

    #[test]
    fn one_image_larger_than_the_ceiling_is_refused() {
        let mut store = ImageStore::with_capacity(100);
        let refused = store.insert(key("big"), image(8), 1.0);
        assert!(matches!(refused, Err(StoreError::Full { limit: 200, .. })));
        assert!(store.is_empty());
    }

    #[test]
    fn trim_drops_down_to_the_target() {
        let mut store = ImageStore::with_capacity(1 << 20);
        for name in ["a", "b", "c"] {
            insert(&mut store, name, 2);
        }
        store.trim(cost(2, "c"));
        assert_eq!((store.len(), store.bytes()), (1, cost(2, "c")));
        assert!(store.lookup(&key("c")).is_some(), "the newest remains");
    }

    #[test]
    fn evicted_ids_are_not_reused() {
        let mut store = ImageStore::with_capacity(cost(2, "a") + 10);
        let a = insert(&mut store, "a", 2);
        let b = insert(&mut store, "b", 2);
        assert!(store.get(a).is_none());
        assert_ne!(a, b);
        assert_eq!(insert(&mut store, "a", 2), ImageId(2));
    }

    #[test]
    fn running_out_of_ids_is_an_error_not_a_wrap() {
        let mut store = ImageStore::with_capacity(1 << 20);
        store.next_id = u64::from(u32::MAX);
        assert_eq!(insert(&mut store, "last", 1), ImageId(u32::MAX));
        assert_eq!(
            store.insert(key("one-too-many"), image(1), 1.0),
            Err(StoreError::IdsExhausted)
        );
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn many_pinned_images_do_not_slow_or_break_eviction() {
        // The recency order holds only unpinned images, so pinned ones are
        // never walked over: this finishes instantly where a scan per
        // eviction would be quadratic.
        let mut store = ImageStore::with_capacity(5_000 * cost(1, "00000"));
        let pinned: Vec<ImageId> = (0..4_000)
            .map(|i| insert(&mut store, &format!("p{i:04}"), 1))
            .collect();
        store.set_pinned(pinned.iter().copied());
        for i in 0..40_000 {
            insert(&mut store, &format!("u{i:05}"), 1);
        }
        assert!(pinned.iter().all(|&id| store.get(id).is_some()));
        assert!(store.bytes() <= 5_000 * cost(1, "00000"));
        assert!(store.len() > 4_000);
    }

    #[test]
    fn nonsense_density_falls_back_to_one() {
        let mut store = ImageStore::with_capacity(1 << 20);
        for (i, density) in [0.0, -1.0, f32::NAN, f32::INFINITY].into_iter().enumerate() {
            let id = store
                .insert(key(&i.to_string()), image(1), density)
                .expect("stored");
            assert_eq!(store.info(id).map(|i| i.density), Some(1.0));
        }
    }
}
