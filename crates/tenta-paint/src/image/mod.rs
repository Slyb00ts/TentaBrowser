//! Image decoding and the decoded-image store.

use std::sync::Arc;

/// Handle to an image in an [`ImageStore`].
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct ImageId(pub u32);

/// Resampling quality for a drawn image.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub enum ImageQuality {
    /// Smooth resampling.
    #[default]
    Smooth,
    /// Nearest neighbour.
    Pixelated,
}

/// A decoded image: premultiplied RGBA8, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba: Arc<[u8]>,
}

/// Decoded images by id. Ids are dense and never reused.
#[derive(Clone, Debug, Default)]
pub struct ImageStore {
    images: Vec<DecodedImage>,
}

impl ImageStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an image and returns its id.
    pub fn insert(&mut self, image: DecodedImage) -> ImageId {
        let id = ImageId(self.images.len() as u32);
        self.images.push(image);
        id
    }

    /// The image with this id.
    pub fn get(&self, id: ImageId) -> Option<&DecodedImage> {
        self.images.get(id.0 as usize)
    }

    /// Number of stored images.
    pub fn len(&self) -> usize {
        self.images.len()
    }

    /// True when the store holds no image.
    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_dense_and_resolve() {
        let mut store = ImageStore::new();
        assert!(store.is_empty());
        let image = DecodedImage {
            width: 1,
            height: 1,
            rgba: Arc::from([0, 0, 0, 255]),
        };
        let a = store.insert(image.clone());
        let b = store.insert(image.clone());
        assert_eq!((a, b), (ImageId(0), ImageId(1)));
        assert_eq!(store.get(b), Some(&image));
        assert_eq!(store.get(ImageId(2)), None);
        assert_eq!(store.len(), 2);
    }
}
