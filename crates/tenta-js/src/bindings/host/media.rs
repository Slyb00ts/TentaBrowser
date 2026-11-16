//! The script interface of media and canvas elements, and the SVG interfaces
//! that have no members of their own: `HTMLMediaElement`, `HTMLVideoElement`,
//! `HTMLAudioElement`, `HTMLCanvasElement`, `SVGGraphicsElement`,
//! `SVGSVGElement`, `SVGAElement`.
//!
//! There is no media pipeline and no canvas backing store, so what is here is
//! the part of each interface that is true without one: the reflected
//! attributes, the state of an element that has loaded nothing, and the two
//! answers that say so — `play()` rejects with `NotSupportedError`, as it does
//! in a browser for an element with no source it can play, and `getContext()`
//! is null, as it is for a context id a browser does not support.

use super::HostObject;
use crate::bindings::{
    AnyValue, Element, HTMLAudioElementOps, HTMLCanvasElementOps, HTMLMediaElementOps,
    HTMLVideoElementOps, Handle, JsError, Promise, SVGAElementOps, SVGGraphicsElementOps,
    SVGSVGElementOps,
};
use crate::dom::events::{HostValue, PromiseOutcome};

impl HostObject {
    /// A reflected non-negative integer attribute, or `default` when it is
    /// absent or does not parse as one.
    fn reflect_unsigned(&self, name: &str, default: u32) -> u32 {
        self.reflect_string(name)
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|value| i32::try_from(*value).is_ok())
            .unwrap_or(default)
    }
}

impl HTMLMediaElementOps for HostObject {
    fn src(&self) -> Result<String, JsError> {
        Ok(self.reflect_url("src"))
    }
    fn set_src(&self, value: &str) {
        self.reflect_attribute("src", value);
    }
    /// Nothing is ever selected as the resource, so there is no current one.
    fn current_src(&self) -> Result<String, JsError> {
        Ok(String::new())
    }
    fn preload(&self) -> Result<String, JsError> {
        // An enumerated attribute: the empty string is `auto`, and anything
        // unknown is the `metadata` default.
        let value = self.reflect_string("preload").to_ascii_lowercase();
        Ok(match value.as_str() {
            "none" | "metadata" | "auto" => value,
            "" if self.reflect_bool("preload") => "auto".to_owned(),
            _ => "metadata".to_owned(),
        })
    }
    fn set_preload(&self, value: &str) {
        self.reflect_attribute("preload", value);
    }
    fn autoplay(&self) -> bool {
        self.reflect_bool("autoplay")
    }
    fn set_autoplay(&self, value: bool) {
        self.set_reflect_bool("autoplay", value);
    }
    fn r#loop(&self) -> bool {
        self.reflect_bool("loop")
    }
    fn set_loop(&self, value: bool) {
        self.set_reflect_bool("loop", value);
    }
    fn controls(&self) -> bool {
        self.reflect_bool("controls")
    }
    fn set_controls(&self, value: bool) {
        self.set_reflect_bool("controls", value);
    }
    /// The element has no playback state of its own to hold a mute flag, so
    /// `muted` follows the content attribute `defaultMuted` reflects.
    fn muted(&self) -> bool {
        self.reflect_bool("muted")
    }
    fn set_muted(&self, value: bool) {
        self.set_reflect_bool("muted", value);
    }
    fn default_muted(&self) -> bool {
        self.reflect_bool("muted")
    }
    fn set_default_muted(&self, value: bool) {
        self.set_reflect_bool("muted", value);
    }
    fn volume(&self) -> f64 {
        1.0
    }
    fn set_volume(&self, _value: f64) {}
    fn current_time(&self) -> f64 {
        0.0
    }
    fn set_current_time(&self, _value: f64) {}
    fn playback_rate(&self) -> f64 {
        1.0
    }
    fn set_playback_rate(&self, _value: f64) {}
    /// `NaN`: no media data is available.
    fn duration(&self) -> f64 {
        f64::NAN
    }
    fn paused(&self) -> bool {
        true
    }
    fn ended(&self) -> bool {
        false
    }
    fn seeking(&self) -> bool {
        false
    }
    /// `HAVE_NOTHING`.
    fn ready_state(&self) -> u16 {
        0
    }
    /// `NETWORK_EMPTY`.
    fn network_state(&self) -> u16 {
        0
    }
    fn play(&self) -> Result<Promise<AnyValue>, JsError> {
        let script = self.script_host()?;
        let promise = script.new_promise().ok_or_else(|| {
            JsError::type_error("HTMLMediaElement.play: the promise could not be created")
        })?;
        script.settle_promise(
            promise.resolver,
            PromiseOutcome::Reject(HostValue::DomException {
                name: "NotSupportedError",
                message: "The element has no supported sources.",
            }),
        );
        Ok(Promise::from_bits(promise.promise))
    }
    fn pause(&self) {}
    fn load(&self) {}
    /// The empty string: this engine can play no type.
    fn can_play_type(&self, _type: &str) -> Result<String, JsError> {
        Ok(String::new())
    }
}

impl HTMLVideoElementOps for HostObject {
    fn width(&self) -> u32 {
        self.reflect_unsigned("width", 0)
    }
    fn set_width(&self, value: u32) {
        self.reflect_attribute("width", &value.to_string());
    }
    fn height(&self) -> u32 {
        self.reflect_unsigned("height", 0)
    }
    fn set_height(&self, value: u32) {
        self.reflect_attribute("height", &value.to_string());
    }
    fn video_width(&self) -> u32 {
        0
    }
    fn video_height(&self) -> u32 {
        0
    }
    fn poster(&self) -> Result<String, JsError> {
        Ok(self.reflect_url("poster"))
    }
    fn set_poster(&self, value: &str) {
        self.reflect_attribute("poster", value);
    }
    fn plays_inline(&self) -> bool {
        self.reflect_bool("playsinline")
    }
    fn set_plays_inline(&self, value: bool) {
        self.set_reflect_bool("playsinline", value);
    }
}

impl HTMLAudioElementOps for HostObject {}

impl HTMLCanvasElementOps for HostObject {
    fn width(&self) -> u32 {
        self.reflect_unsigned("width", 300)
    }
    fn set_width(&self, value: u32) {
        self.reflect_attribute("width", &value.to_string());
    }
    fn height(&self) -> u32 {
        self.reflect_unsigned("height", 150)
    }
    fn set_height(&self, value: u32) {
        self.reflect_attribute("height", &value.to_string());
    }
    /// Null for every context id: none is supported.
    fn get_context(&self, _context_id: &str) -> Result<Option<Handle<Element>>, JsError> {
        Ok(None)
    }
    /// What a canvas with no pixels serializes to.
    fn to_data_url(&self) -> Result<String, JsError> {
        Ok("data:,".to_owned())
    }
}

impl SVGGraphicsElementOps for HostObject {}
impl SVGSVGElementOps for HostObject {}
impl SVGAElementOps for HostObject {}

impl crate::bindings::HTMLMetaElementOps for HostObject {
    fn name(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("name"))
    }
    fn set_name(&self, value: &str) {
        self.reflect_attribute("name", value);
    }
    fn http_equiv(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("http-equiv"))
    }
    fn set_http_equiv(&self, value: &str) {
        self.reflect_attribute("http-equiv", value);
    }
    fn content(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("content"))
    }
    fn set_content(&self, value: &str) {
        self.reflect_attribute("content", value);
    }
    fn media(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("media"))
    }
    fn set_media(&self, value: &str) {
        self.reflect_attribute("media", value);
    }
}

impl crate::bindings::HTMLSourceElementOps for HostObject {
    fn src(&self) -> Result<String, JsError> {
        Ok(self.reflect_url("src"))
    }
    fn set_src(&self, value: &str) {
        self.reflect_attribute("src", value);
    }
    fn r#type(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("type"))
    }
    fn set_type(&self, value: &str) {
        self.reflect_attribute("type", value);
    }
    fn srcset(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("srcset"))
    }
    fn set_srcset(&self, value: &str) {
        self.reflect_attribute("srcset", value);
    }
    fn sizes(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("sizes"))
    }
    fn set_sizes(&self, value: &str) {
        self.reflect_attribute("sizes", value);
    }
    fn media(&self) -> Result<String, JsError> {
        Ok(self.reflect_string("media"))
    }
    fn set_media(&self, value: &str) {
        self.reflect_attribute("media", value);
    }
}

/// A dialog is shown by its `open` attribute and nothing else: there is no
/// top layer to put a modal one in, and closing fires no event.
impl crate::bindings::HTMLDialogElementOps for HostObject {
    fn open(&self) -> bool {
        self.reflect_bool("open")
    }
    fn set_open(&self, value: bool) {
        self.set_reflect_bool("open", value);
    }
    fn show(&self) {
        self.set_reflect_bool("open", true);
    }
    fn show_modal(&self) {
        self.set_reflect_bool("open", true);
    }
    fn close(&self) {
        self.set_reflect_bool("open", false);
    }
}
