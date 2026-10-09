//! Value types shared by layout, paint and render.
//!
//! Everything a [`crate::Fragment`] or a [`PaintStyle`] mentions lives here and
//! is free of Stylo, so `tenta-paint` (which may depend on this crate but not
//! the reverse) and hand-built fragment trees in tests never touch
//! `ComputedValues`. `tenta-paint` re-exports these names.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

// ---------------------------------------------------------------- geometry

/// An axis-aligned rectangle in CSS pixels. Layout produces it, paint and
/// geometry consume it.
#[derive(Copy, Clone, Default, PartialEq, Debug)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    /// A rectangle from its origin and size.
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// The x coordinate of the right edge.
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    /// The y coordinate of the bottom edge.
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// The same rectangle moved by `(dx, dy)`.
    pub fn translated(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    /// The smallest rectangle containing both. An empty operand (zero width and
    /// height) is ignored so accumulating from `Rect::default()` is safe.
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_zero() {
            return *other;
        }
        if other.is_zero() {
            return *self;
        }
        let left = self.x.min(other.x);
        let top = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Rect::new(left, top, right - left, bottom - top)
    }

    /// True when the point lies inside the half-open rectangle.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    fn is_zero(&self) -> bool {
        self.w == 0.0 && self.h == 0.0
    }
}

/// Four per-side lengths in CSS pixels (border, padding or margin widths).
#[derive(Copy, Clone, Default, PartialEq, Debug)]
pub struct Edges {
    /// Top side.
    pub top: f32,
    /// Right side.
    pub right: f32,
    /// Bottom side.
    pub bottom: f32,
    /// Left side.
    pub left: f32,
}

impl Edges {
    /// The same length on every side.
    pub const fn all(v: f32) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    /// Left plus right.
    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    /// Top plus bottom.
    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

/// A CSS `<length-percentage>` that cannot be resolved until the box size is
/// known: `px + fraction * basis`. Layout stores it in [`PaintStyle`] and paint
/// resolves it against the box it paints.
#[derive(Copy, Clone, Default, PartialEq, Debug)]
pub struct LengthPercent {
    /// Absolute part in CSS pixels.
    pub px: f32,
    /// Percentage part as a fraction (`0.5` is 50%).
    pub fraction: f32,
}

impl LengthPercent {
    /// A purely absolute length.
    pub const fn px(px: f32) -> Self {
        Self { px, fraction: 0.0 }
    }

    /// A purely relative length (`0.5` is 50%).
    pub const fn fraction(fraction: f32) -> Self {
        Self { px: 0.0, fraction }
    }

    /// The length against `basis` pixels.
    pub fn resolve(&self, basis: f32) -> f32 {
        self.px + self.fraction * basis
    }
}

/// A 2D affine transform `[a b c d e f]` mapping `(x, y)` to
/// `(a*x + c*y + e, b*x + d*y + f)`.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Affine {
    /// Horizontal scale.
    pub a: f32,
    /// Vertical skew.
    pub b: f32,
    /// Horizontal skew.
    pub c: f32,
    /// Vertical scale.
    pub d: f32,
    /// Horizontal translation.
    pub e: f32,
    /// Vertical translation.
    pub f: f32,
}

impl Default for Affine {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Affine {
    /// The identity transform.
    pub const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    /// A translation.
    pub const fn translation(x: f32, y: f32) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: x,
            f: y,
        }
    }

    /// A scale about the origin.
    pub const fn scale(x: f32, y: f32) -> Self {
        Self {
            a: x,
            b: 0.0,
            c: 0.0,
            d: y,
            e: 0.0,
            f: 0.0,
        }
    }

    /// `self` applied after `first` (matrix product `self * first`).
    pub fn after(&self, first: &Affine) -> Affine {
        Affine {
            a: self.a * first.a + self.c * first.b,
            b: self.b * first.a + self.d * first.b,
            c: self.a * first.c + self.c * first.d,
            d: self.b * first.c + self.d * first.d,
            e: self.a * first.e + self.c * first.f + self.e,
            f: self.b * first.e + self.d * first.f + self.f,
        }
    }

    /// Maps a point.
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// True for the identity transform.
    pub fn is_identity(&self) -> bool {
        *self == Affine::IDENTITY
    }

    /// The matrix as `[a, b, c, d, e, f]`, the layout `PushTransform` carries.
    pub fn to_array(&self) -> [f32; 6] {
        [self.a, self.b, self.c, self.d, self.e, self.f]
    }
}

// ---------------------------------------------------------------- colour

/// A premultiplied sRGB colour with `f32` channels in `0.0..=1.0`. Layout
/// converts Stylo colours once while building [`PaintStyle`]; paint never
/// sees a Stylo colour.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Color {
    /// Red, premultiplied by alpha.
    pub r: f32,
    /// Green, premultiplied by alpha.
    pub g: f32,
    /// Blue, premultiplied by alpha.
    pub b: f32,
    /// Alpha.
    pub a: f32,
}

impl Default for Color {
    fn default() -> Self {
        Self::TRANSPARENT
    }
}

impl Color {
    /// Fully transparent black, the initial `background-color`.
    pub const TRANSPARENT: Color = Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };
    /// Opaque black, the initial `color`.
    pub const BLACK: Color = Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    /// Opaque white.
    pub const WHITE: Color = Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    };

    /// Builds a premultiplied colour from straight (non-premultiplied) 8-bit
    /// channels, the form CSS authors write.
    pub fn from_rgba8(r: u8, g: u8, b: u8, a: u8) -> Color {
        let alpha = f32::from(a) / 255.0;
        Color {
            r: f32::from(r) / 255.0 * alpha,
            g: f32::from(g) / 255.0 * alpha,
            b: f32::from(b) / 255.0 * alpha,
            a: alpha,
        }
    }

    /// Premultiplied 8-bit channels `[r, g, b, a]`, rounded.
    pub fn to_premultiplied_rgba8(&self) -> [u8; 4] {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        [q(self.r), q(self.g), q(self.b), q(self.a)]
    }

    /// True when nothing would be painted.
    pub fn is_transparent(&self) -> bool {
        self.a <= 0.0
    }

    /// The colour with its coverage multiplied by `opacity`.
    pub fn with_opacity(&self, opacity: f32) -> Color {
        Color {
            r: self.r * opacity,
            g: self.g * opacity,
            b: self.b * opacity,
            a: self.a * opacity,
        }
    }
}

// ---------------------------------------------------------------- paint enums

/// CSS `border-style`.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub enum BorderStyle {
    /// No border; the used width is zero.
    #[default]
    None,
    /// Like `None`, but wins conflicts in collapsed table borders.
    Hidden,
    /// A solid line.
    Solid,
    /// Dashes.
    Dashed,
    /// Dots.
    Dotted,
    /// Two lines with a gap.
    Double,
    /// Carved into the page.
    Groove,
    /// Coming out of the page.
    Ridge,
    /// The box looks embedded.
    Inset,
    /// The box looks embossed.
    Outset,
}

/// The line style of a text decoration.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub enum LineStyle {
    /// A solid line.
    #[default]
    Solid,
    /// Two lines.
    Double,
    /// Dots.
    Dotted,
    /// Dashes.
    Dashed,
    /// A wave.
    Wavy,
}

/// CSS `mix-blend-mode` / `background-blend-mode`.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub enum BlendMode {
    /// Source over, no blending.
    #[default]
    Normal,
    /// `multiply`.
    Multiply,
    /// `screen`.
    Screen,
    /// `overlay`.
    Overlay,
    /// `darken`.
    Darken,
    /// `lighten`.
    Lighten,
    /// `color-dodge`.
    ColorDodge,
    /// `color-burn`.
    ColorBurn,
    /// `hard-light`.
    HardLight,
    /// `soft-light`.
    SoftLight,
    /// `difference`.
    Difference,
    /// `exclusion`.
    Exclusion,
    /// `hue`.
    Hue,
    /// `saturation`.
    Saturation,
    /// `color`.
    Color,
    /// `luminosity`.
    Luminosity,
}

/// One function of the CSS `filter` list, arguments resolved to numbers.
#[derive(Clone, PartialEq, Debug)]
pub enum FilterOp {
    /// Gaussian blur with this standard deviation in CSS pixels.
    Blur(f32),
    /// `brightness()` factor.
    Brightness(f32),
    /// `contrast()` factor.
    Contrast(f32),
    /// `grayscale()` amount in `0..=1`.
    Grayscale(f32),
    /// `hue-rotate()` in degrees.
    HueRotate(f32),
    /// `invert()` amount in `0..=1`.
    Invert(f32),
    /// `opacity()` amount in `0..=1`.
    Opacity(f32),
    /// `saturate()` factor.
    Saturate(f32),
    /// `sepia()` amount in `0..=1`.
    Sepia(f32),
    /// `drop-shadow()`.
    DropShadow(ShadowPaint),
}

/// CSS `object-fit`.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub enum ObjectFit {
    /// Stretch to the content box.
    #[default]
    Fill,
    /// Scale to fit, keeping the aspect ratio.
    Contain,
    /// Scale to cover, keeping the aspect ratio.
    Cover,
    /// Natural size.
    None,
    /// `contain` or `none`, whichever is smaller.
    ScaleDown,
}

/// CSS `image-rendering`, reduced to what the rasteriser distinguishes.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub enum ImageRendering {
    /// Smooth resampling.
    #[default]
    Auto,
    /// Nearest-neighbour (`pixelated` and `crisp-edges`).
    Pixelated,
}

/// CSS `overflow-x` / `overflow-y`.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub enum OverflowKind {
    /// Not clipped.
    #[default]
    Visible,
    /// Clipped, programmatically scrollable.
    Hidden,
    /// Clipped, not scrollable.
    Clip,
    /// Always a scroll container.
    Scroll,
    /// A scroll container when content overflows.
    Auto,
}

/// CSS `position`.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub enum PositionKind {
    /// Normal flow.
    #[default]
    Static,
    /// Normal flow, then offset.
    Relative,
    /// Out of flow, against the containing block.
    Absolute,
    /// Out of flow, against the viewport.
    Fixed,
    /// Relative until a scroll threshold.
    Sticky,
}

/// One of the three CSS boxes a background is positioned or clipped against.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub enum BoxArea {
    /// The border box.
    #[default]
    BorderBox,
    /// The padding box.
    PaddingBox,
    /// The content box.
    ContentBox,
}

/// `background-repeat` along one axis.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub enum RepeatKind {
    /// Tiled.
    #[default]
    Repeat,
    /// Drawn once.
    NoRepeat,
    /// Tiled, scaled to fit a whole number of tiles.
    Round,
    /// Tiled, spaced to fit a whole number of tiles.
    Space,
}

/// `background-size`.
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum BackgroundSize {
    /// The image's own size.
    Auto,
    /// Scale to cover the positioning area.
    Cover,
    /// Scale to fit inside the positioning area.
    Contain,
    /// Explicit width and height; `None` is `auto` on that axis.
    Explicit(Option<LengthPercent>, Option<LengthPercent>),
}

/// The key under which an image resource is requested: the resolved absolute
/// URL. Layout fills it, the image store of `tenta-paint` looks it up.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ImageKey(pub Arc<str>);

/// A colour stop of a gradient.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct ColorStop {
    /// Position along the gradient line.
    pub position: LengthPercent,
    /// Colour at that position.
    pub color: Color,
}

/// A linear gradient (`linear-gradient()` and its repeating form).
#[derive(Clone, PartialEq, Debug)]
pub struct GradientPaint {
    /// Direction in degrees, `0` pointing up, clockwise.
    pub angle_deg: f32,
    /// Stops in specified order.
    pub stops: Vec<ColorStop>,
    /// `repeating-linear-gradient()`.
    pub repeating: bool,
}

/// What a background layer draws.
#[derive(Clone, PartialEq, Debug)]
pub enum BackgroundContent {
    /// A flat colour; always the last layer.
    Color(Color),
    /// An image fetched by URL.
    Image(ImageKey),
    /// A gradient.
    Gradient(GradientPaint),
}

/// One `background-*` layer.
#[derive(Clone, PartialEq, Debug)]
pub struct BackgroundLayer {
    /// The painted content.
    pub content: BackgroundContent,
    /// Repeat along x and y.
    pub repeat: (RepeatKind, RepeatKind),
    /// Position in the positioning area.
    pub position: (LengthPercent, LengthPercent),
    /// Tile size.
    pub size: BackgroundSize,
    /// Box that anchors position and size.
    pub origin: BoxArea,
    /// Box the layer is clipped to.
    pub clip: BoxArea,
}

/// Border widths, colours, styles and radii of one box.
#[derive(Clone, PartialEq, Debug)]
pub struct BorderPaint {
    /// Used widths (zero where the style is `none` or `hidden`).
    pub widths: Edges,
    /// Colours in top, right, bottom, left order.
    pub colors: [Color; 4],
    /// Styles in top, right, bottom, left order.
    pub styles: [BorderStyle; 4],
    /// Corner radii as `(x, y)` pairs in top-left, top-right, bottom-right,
    /// bottom-left order. Percentages stay unresolved because a [`PaintStyle`] is
    /// shared by every fragment of a box; paint resolves and clamps them against
    /// the fragment's border box.
    pub radii: [LengthPercent; 8],
}

impl Default for BorderPaint {
    fn default() -> Self {
        Self {
            widths: Edges::default(),
            colors: [Color::BLACK; 4],
            styles: [BorderStyle::None; 4],
            radii: [LengthPercent::default(); 8],
        }
    }
}

/// An `outline`.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct OutlinePaint {
    /// Line width.
    pub width: f32,
    /// Line style.
    pub style: BorderStyle,
    /// Line colour.
    pub color: Color,
    /// Distance from the border edge (`outline-offset`).
    pub offset: f32,
}

/// A `box-shadow`, `text-shadow` or `drop-shadow()`.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct ShadowPaint {
    /// Horizontal and vertical offset.
    pub offset: (f32, f32),
    /// Blur radius.
    pub blur: f32,
    /// Spread radius (zero for text shadows).
    pub spread: f32,
    /// Shadow colour.
    pub color: Color,
    /// `inset` box shadow.
    pub inset: bool,
}

/// `text-decoration` as paint needs it.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct DecorationPaint {
    /// `underline`.
    pub underline: bool,
    /// `overline`.
    pub overline: bool,
    /// `line-through`.
    pub line_through: bool,
    /// Line style.
    pub style: LineStyle,
    /// Line colour (`currentcolor` already resolved).
    pub color: Color,
    /// Explicit thickness; `None` is `auto`, a tenth of the font size.
    pub thickness: Option<f32>,
    /// `text-underline-offset` added to the underline's position; `None` is
    /// `auto`. A percentage is a fraction of the font size.
    pub underline_offset: Option<LengthPercent>,
    /// `text-underline-position`.
    pub underline_position: UnderlinePosition,
    /// `text-decoration-skip-ink` is `auto` (the initial value), so the
    /// underline should break around descenders. `false` for `none`.
    pub skip_ink: bool,
}

/// Where an underline sits (`text-underline-position`).
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub enum UnderlinePosition {
    /// Just below the baseline.
    #[default]
    Auto,
    /// At the position the font's `post` table suggests.
    FromFont,
    /// Below the font's descent.
    Under,
}

impl Default for DecorationPaint {
    fn default() -> Self {
        Self {
            underline: false,
            overline: false,
            line_through: false,
            style: LineStyle::Solid,
            color: Color::BLACK,
            thickness: None,
            underline_offset: None,
            underline_position: UnderlinePosition::Auto,
            skip_ink: true,
        }
    }
}

/// How a list marker is drawn.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub enum MarkerKind {
    /// Filled circle.
    #[default]
    Disc,
    /// Hollow circle.
    Circle,
    /// Filled square.
    Square,
    /// Triangle for an open `<details>`.
    DisclosureOpen,
    /// Triangle for a closed `<details>`.
    DisclosureClosed,
    /// A counter string such as `1.`, laid out as text.
    Text,
}

/// `list-style` as paint needs it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct MarkerPaint {
    /// Marker shape.
    pub kind: MarkerKind,
    /// `list-style-position: inside`.
    pub inside: bool,
}

/// Everything paint needs from style, resolved to plain values by the box-tree
/// builder. One `Arc<PaintStyle>` is shared by every fragment of a box.
#[derive(Clone, PartialEq, Debug)]
pub struct PaintStyle {
    /// `visibility: hidden` or `collapse`.
    pub visibility_hidden: bool,
    /// `opacity`.
    pub opacity: f32,
    /// `pointer-events` is not `none`.
    pub pointer_events: bool,
    /// Background layers, topmost first; the colour layer is last.
    pub background: Vec<BackgroundLayer>,
    /// Border.
    pub border: BorderPaint,
    /// Outline.
    pub outline: Option<OutlinePaint>,
    /// `box-shadow` list, topmost first.
    pub box_shadow: Vec<ShadowPaint>,
    /// `text-shadow` list, topmost first.
    pub text_shadow: Vec<ShadowPaint>,
    /// Foreground `color`.
    pub color: Color,
    /// `text-decoration`.
    pub text_decoration: DecorationPaint,
    /// `overflow-x`.
    pub overflow_x: OverflowKind,
    /// `overflow-y`.
    pub overflow_y: OverflowKind,
    /// `position`.
    pub position: PositionKind,
    /// `z-index`; `None` is `auto`.
    pub z_index: Option<i32>,
    /// 2D `transform`; `None` when it is the identity.
    pub transform: Option<Affine>,
    /// `transform-origin`, resolved against the border box at paint time.
    pub transform_origin: (LengthPercent, LengthPercent),
    /// `filter` list.
    pub filter: Vec<FilterOp>,
    /// `mix-blend-mode`.
    pub blend: BlendMode,
    /// `isolation: isolate`.
    pub isolate: bool,
    /// List marker, set on list items only.
    pub list_marker: Option<MarkerPaint>,
    /// `object-fit` for replaced content.
    pub object_fit: ObjectFit,
    /// `object-position`, resolved against the content box at paint time.
    pub object_position: (LengthPercent, LengthPercent),
    /// `image-rendering`.
    pub image_rendering: ImageRendering,
}

impl Default for PaintStyle {
    fn default() -> Self {
        let center = (LengthPercent::fraction(0.5), LengthPercent::fraction(0.5));
        Self {
            visibility_hidden: false,
            opacity: 1.0,
            pointer_events: true,
            background: Vec::new(),
            border: BorderPaint::default(),
            outline: None,
            box_shadow: Vec::new(),
            text_shadow: Vec::new(),
            color: Color::BLACK,
            text_decoration: DecorationPaint::default(),
            overflow_x: OverflowKind::Visible,
            overflow_y: OverflowKind::Visible,
            position: PositionKind::Static,
            z_index: None,
            transform: None,
            transform_origin: center,
            filter: Vec::new(),
            blend: BlendMode::Normal,
            isolate: false,
            list_marker: None,
            object_fit: ObjectFit::Fill,
            object_position: center,
            image_rendering: ImageRendering::Auto,
        }
    }
}

// ---------------------------------------------------------------- fonts and glyphs

/// The bytes of one font file. Embedded fonts are `'static`; fonts that arrive
/// later (web fonts) are owned.
pub enum FontBlob {
    /// Bytes compiled into the binary.
    Static(&'static [u8]),
    /// Bytes owned by the blob.
    Owned(Box<[u8]>),
}

impl FontBlob {
    /// The font file.
    pub fn bytes(&self) -> &[u8] {
        match self {
            FontBlob::Static(bytes) => bytes,
            FontBlob::Owned(bytes) => bytes,
        }
    }
}

impl std::fmt::Debug for FontBlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FontBlob({} bytes)", self.bytes().len())
    }
}

/// A face inside a font file at specific variation coordinates. Identity is
/// the blob's address, the collection index and the coordinates, so glyph
/// caches can key on it without hashing the font.
#[derive(Clone, Debug)]
pub struct FontRef {
    /// The font file.
    pub data: Arc<FontBlob>,
    /// Face index inside a collection.
    pub index: u32,
    /// Normalised variation coordinates (F2Dot14), empty for static fonts.
    pub coords: Arc<[i16]>,
}

impl PartialEq for FontRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.data, &other.data)
            && self.index == other.index
            && self.coords == other.coords
    }
}

impl Eq for FontRef {}

impl Hash for FontRef {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.data).hash(state);
        self.index.hash(state);
        self.coords.hash(state);
    }
}

/// One positioned glyph, relative to the origin of its text fragment.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Glyph {
    /// Glyph id in the font.
    pub id: u32,
    /// Horizontal position.
    pub x: f32,
    /// Vertical position (baseline-relative).
    pub y: f32,
}

/// A run of glyphs in one font at one size.
#[derive(Clone, PartialEq, Debug)]
pub struct GlyphRun {
    /// The face.
    pub font: FontRef,
    /// Font size in CSS pixels.
    pub size: f32,
    /// Positioned glyphs.
    pub glyphs: Vec<Glyph>,
    /// Embolden outlines because no bold face was matched.
    pub synthetic_bold: bool,
    /// Horizontal skew (tangent) for a synthetic italic.
    pub skew: f32,
}

// ---------------------------------------------------------------- layout enums

/// Inline base direction of a containing block (`direction`).
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub enum Direction {
    /// Left to right.
    #[default]
    Ltr,
    /// Right to left.
    Rtl,
}

/// `writing-mode`. Only [`WritingMode::HorizontalTb`] is laid out.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub enum WritingMode {
    /// Horizontal text, blocks stack top to bottom.
    #[default]
    HorizontalTb,
    /// Vertical text, blocks stack right to left.
    VerticalRl,
    /// Vertical text, blocks stack left to right.
    VerticalLr,
}

/// Side a float is placed on.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum FloatSide {
    /// `float: left`.
    Left,
    /// `float: right`.
    Right,
}

/// CSS `clear`.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub enum ClearSide {
    /// No clearing.
    #[default]
    None,
    /// Clear left floats.
    Left,
    /// Clear right floats.
    Right,
    /// Clear both.
    Both,
}

/// Kind of a replaced box or fragment.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ReplacedKind {
    /// `<img>` and images from `content`.
    Image,
    /// `<canvas>`.
    Canvas,
    /// `<svg>`.
    Svg,
    /// `<video>`.
    Video,
    /// `<iframe>`, `<object>`, `<embed>`.
    Iframe,
    /// A form control painted by the engine.
    Control(ControlKind),
}

/// Form controls with an engine-painted appearance.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ControlKind {
    /// Single-line text input.
    TextInput,
    /// `<textarea>`.
    TextArea,
    /// Checkbox.
    Checkbox,
    /// Radio button.
    Radio,
    /// Push button, `<button>` and button-like inputs.
    Button,
    /// `<select>`.
    Select,
    /// `<input type=range>`.
    Range,
    /// `<progress>`.
    Progress,
    /// `<meter>`.
    Meter,
    /// `<input type=file>`.
    File,
    /// `<input type=color>`.
    Color,
}

/// Role of a box in a table.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum TableKind {
    /// The table box.
    Table,
    /// `table-caption`.
    Caption,
    /// Header, body or footer group.
    RowGroup,
    /// `table-row`.
    Row,
    /// `table-cell`.
    Cell,
    /// `table-column`.
    Column,
    /// `table-column-group`.
    ColumnGroup,
}

/// Intrinsic dimensions of a replaced element's content.
#[derive(Copy, Clone, Default, PartialEq, Debug)]
pub struct IntrinsicSize {
    /// Natural width.
    pub width: Option<f32>,
    /// Natural height.
    pub height: Option<f32>,
    /// Natural aspect ratio (width over height).
    pub ratio: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_union_ignores_empty_operands() {
        let a = Rect::new(10.0, 10.0, 20.0, 5.0);
        assert_eq!(Rect::default().union(&a), a);
        assert_eq!(a.union(&Rect::default()), a);
        assert_eq!(
            a.union(&Rect::new(0.0, 12.0, 5.0, 20.0)),
            Rect::new(0.0, 10.0, 30.0, 22.0)
        );
    }

    #[test]
    fn rect_contains_is_half_open() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains(0.0, 0.0));
        assert!(!r.contains(10.0, 5.0));
        assert!(!r.contains(5.0, 10.0));
        assert_eq!(r.translated(2.0, 3.0), Rect::new(2.0, 3.0, 10.0, 10.0));
    }

    #[test]
    fn edges_sum_per_axis() {
        let e = Edges {
            top: 1.0,
            right: 2.0,
            bottom: 3.0,
            left: 4.0,
        };
        assert_eq!(e.horizontal(), 6.0);
        assert_eq!(e.vertical(), 4.0);
        assert_eq!(Edges::all(2.0).vertical(), 4.0);
    }

    #[test]
    fn length_percent_resolves_against_a_basis() {
        let l = LengthPercent {
            px: 4.0,
            fraction: 0.25,
        };
        assert_eq!(l.resolve(100.0), 29.0);
        assert_eq!(LengthPercent::px(3.0).resolve(50.0), 3.0);
        assert_eq!(LengthPercent::fraction(0.5).resolve(50.0), 25.0);
    }

    #[test]
    fn affine_composes_and_maps_points() {
        let t = Affine::translation(10.0, 20.0);
        let s = Affine::scale(2.0, 3.0);
        // Scale first, then translate.
        assert_eq!(t.after(&s).apply(1.0, 1.0), (12.0, 23.0));
        // Translate first, then scale.
        assert_eq!(s.after(&t).apply(1.0, 1.0), (22.0, 63.0));
        assert!(Affine::default().is_identity());
        assert!(!t.is_identity());
        assert_eq!(t.to_array(), [1.0, 0.0, 0.0, 1.0, 10.0, 20.0]);
    }

    #[test]
    fn color_is_premultiplied() {
        let c = Color::from_rgba8(255, 0, 0, 128);
        assert!((c.a - 128.0 / 255.0).abs() < 1e-6);
        assert!((c.r - c.a).abs() < 1e-6);
        assert_eq!(c.to_premultiplied_rgba8(), [128, 0, 0, 128]);
        assert!(Color::TRANSPARENT.is_transparent());
        assert!(!Color::BLACK.is_transparent());
        assert_eq!(Color::WHITE.with_opacity(0.5).a, 0.5);
        assert_eq!(Color::default(), Color::TRANSPARENT);
    }

    #[test]
    fn paint_style_default_is_the_css_initial_state() {
        let s = PaintStyle::default();
        assert_eq!(s.opacity, 1.0);
        assert!(s.pointer_events);
        assert!(!s.visibility_hidden);
        assert_eq!(s.color, Color::BLACK);
        assert_eq!(s.position, PositionKind::Static);
        assert_eq!(s.z_index, None);
        assert!(s.background.is_empty());
        assert_eq!(s.border.styles, [BorderStyle::None; 4]);
        assert_eq!(s.transform_origin.0.resolve(200.0), 100.0);
        assert_eq!(s.object_position.1.resolve(40.0), 20.0);
        assert_eq!(s, s.clone());
    }

    #[test]
    fn font_ref_identity_is_blob_address_index_and_coords() {
        static BYTES: [u8; 4] = [0, 1, 0, 0];
        let blob = Arc::new(FontBlob::Static(&BYTES));
        let a = FontRef {
            data: blob.clone(),
            index: 0,
            coords: Arc::from([]),
        };
        let same = FontRef {
            data: blob,
            index: 0,
            coords: Arc::from([]),
        };
        let other_blob = FontRef {
            data: Arc::new(FontBlob::Static(&BYTES)),
            index: 0,
            coords: Arc::from([]),
        };
        let other_coords = FontRef {
            coords: Arc::from([1_i16]),
            ..same.clone()
        };
        assert_eq!(a, same);
        assert_ne!(a, other_blob);
        assert_ne!(a, other_coords);

        let hash = |f: &FontRef| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            f.hash(&mut h);
            h.finish()
        };
        assert_eq!(hash(&a), hash(&same));
        assert_eq!(a.data.bytes(), &BYTES);
        assert_eq!(FontBlob::Owned(Box::new([7])).bytes(), &[7]);
    }
}
