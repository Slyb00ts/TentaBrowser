//! DOM binding glue generated from `idl/dom`, `idl/url`, `idl/fetch`,
//! `idl/encoding`, `idl/xhr` and `idl/cssom` by `tenta-idl` (see `build.rs`).
//!
//! The engine-independent output (`prelude.rs`, `types.rs`, the `*Ops` traits
//! and the `register_*` functions) is always compiled. The V8 glue (`*_v8.rs`)
//! and QuickJS glue (`*_qjs.rs`) are compiled only behind their features, which
//! pull the corresponding engine crate.
//!
//! Everything is included into this one module so the generated files see each
//! other and the shared prelude. Dead code is expected until each interface is
//! wired to a host, so the generated items are allowed to be unused. A
//! member-less interface's V8 installer also builds a `Signature` it does not
//! use, so `unused_variables` is allowed for the generated glue too.
#![allow(dead_code)]
#![allow(unused_variables)]
// A member-less interface's generated `construct_*` throw stub carries the host
// type parameter for signature uniformity but never names it.
#![allow(clippy::extra_unused_type_parameters)]
// A fast getter/operation whose return type is already the V8 fast width (e.g.
// an `unsigned long` returning `u32`) still emits the widening `as` cast for
// uniformity, so the cast is a no-op the generator does not special-case.
#![allow(clippy::unnecessary_cast)]
// A void operation's receiver-dispatch glue is emitted as a uniform two-arm
// `match` (`Some(host) => host.op(..), None => ()`); the generator does not
// special-case the void arm into an `if let`.
#![allow(clippy::single_match)]
// A nullable-`DOMString` member wraps its value through `Ok(..).map(Some)` for
// uniformity with the fallible-conversion path, which clippy reads as a
// redundant map on the `Ok` constructor.
#![allow(clippy::unnecessary_map_on_constructor)]
// A dictionary's `from_v8` reads each member with `self.member.clone()` for
// uniformity across member types; for a `Copy` member (a `boolean` or an `any`)
// clippy reads the clone as redundant.
#![allow(clippy::clone_on_copy)]
// A nullable member wrapped in an optional (`Option<Option<T>>`: present, then
// nullable) serializes through a uniform outer/inner `match`; clippy reads the
// inner `match` as collapsible into the outer one.
#![allow(clippy::collapsible_match)]

include!(concat!(env!("OUT_DIR"), "/prelude.rs"));
include!(concat!(env!("OUT_DIR"), "/types.rs"));

include!(concat!(env!("OUT_DIR"), "/character_data_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/event_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/custom_event_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/ui_event_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/composition_event_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/focus_event_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/keyboard_event_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/mouse_event_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/wheel_event_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/progress_event_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/htmli_frame_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/svg_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/document_fragment_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_template_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/event_target_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/comment_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/document_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/document_type_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/processing_instruction_ops.rs"));
// `CDATASection : Text` and the three range interfaces, which root at
// `Object`. Their order here is the order the file lists interfaces in, not an
// install order: `crates/tenta-js-v8/src/lib.rs` installs parents first.
include!(concat!(env!("OUT_DIR"), "/cdata_section_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/abstract_range_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/range_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/static_range_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/mutation_observer_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/mutation_record_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/dom_implementation_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/dom_parser_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/xml_serializer_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_collection_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/node_list_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/named_node_map_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/attr_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/dom_token_list_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/dom_string_map_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/style_sheet_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/css_style_sheet_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/css_rule_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/css_grouping_rule_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/css_style_rule_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/css_rule_list_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/css_style_declaration_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/css_style_properties_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/storage_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/crypto_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/css_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_div_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_span_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_paragraph_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_media_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_video_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_audio_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_canvas_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_meta_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_source_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_dialog_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_slot_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/shadow_root_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/svg_graphics_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/svgsvg_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/svga_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_anchor_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_image_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_label_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_form_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_input_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_button_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_select_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_text_area_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_option_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_script_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_link_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/html_style_element_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/dom_rect_read_only_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/dom_rect_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/dom_rect_list_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/style_sheet_list_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/node_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/text_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/url_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/url_search_params_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/headers_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/abort_signal_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/abort_controller_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/text_decoder_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/text_encoder_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/response_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/request_ops.rs"));
include!(concat!(
    env!("OUT_DIR"),
    "/xml_http_request_event_target_ops.rs"
));
include!(concat!(env!("OUT_DIR"), "/xml_http_request_ops.rs"));

include!(concat!(env!("OUT_DIR"), "/character_data_register.rs"));
include!(concat!(env!("OUT_DIR"), "/event_register.rs"));
include!(concat!(env!("OUT_DIR"), "/custom_event_register.rs"));
include!(concat!(env!("OUT_DIR"), "/ui_event_register.rs"));
include!(concat!(env!("OUT_DIR"), "/composition_event_register.rs"));
include!(concat!(env!("OUT_DIR"), "/focus_event_register.rs"));
include!(concat!(env!("OUT_DIR"), "/keyboard_event_register.rs"));
include!(concat!(env!("OUT_DIR"), "/mouse_event_register.rs"));
include!(concat!(env!("OUT_DIR"), "/wheel_event_register.rs"));
include!(concat!(env!("OUT_DIR"), "/progress_event_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/htmli_frame_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/svg_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/document_fragment_register.rs"));
include!(concat!(
    env!("OUT_DIR"),
    "/html_template_element_register.rs"
));
include!(concat!(env!("OUT_DIR"), "/event_target_register.rs"));
include!(concat!(env!("OUT_DIR"), "/comment_register.rs"));
include!(concat!(env!("OUT_DIR"), "/document_register.rs"));
include!(concat!(env!("OUT_DIR"), "/document_type_register.rs"));
include!(concat!(
    env!("OUT_DIR"),
    "/processing_instruction_register.rs"
));
include!(concat!(env!("OUT_DIR"), "/dom_implementation_register.rs"));
include!(concat!(env!("OUT_DIR"), "/dom_parser_register.rs"));
include!(concat!(env!("OUT_DIR"), "/xml_serializer_register.rs"));
include!(concat!(env!("OUT_DIR"), "/element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_collection_register.rs"));
include!(concat!(env!("OUT_DIR"), "/node_list_register.rs"));
include!(concat!(env!("OUT_DIR"), "/named_node_map_register.rs"));
include!(concat!(env!("OUT_DIR"), "/attr_register.rs"));
include!(concat!(env!("OUT_DIR"), "/dom_token_list_register.rs"));
include!(concat!(env!("OUT_DIR"), "/dom_string_map_register.rs"));
include!(concat!(env!("OUT_DIR"), "/style_sheet_register.rs"));
include!(concat!(env!("OUT_DIR"), "/css_style_sheet_register.rs"));
include!(concat!(env!("OUT_DIR"), "/css_rule_register.rs"));
include!(concat!(env!("OUT_DIR"), "/css_grouping_rule_register.rs"));
include!(concat!(env!("OUT_DIR"), "/css_style_rule_register.rs"));
include!(concat!(env!("OUT_DIR"), "/css_rule_list_register.rs"));
include!(concat!(
    env!("OUT_DIR"),
    "/css_style_declaration_register.rs"
));
include!(concat!(
    env!("OUT_DIR"),
    "/css_style_properties_register.rs"
));
include!(concat!(env!("OUT_DIR"), "/storage_register.rs"));
include!(concat!(env!("OUT_DIR"), "/crypto_register.rs"));
include!(concat!(env!("OUT_DIR"), "/css_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_div_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_span_element_register.rs"));
include!(concat!(
    env!("OUT_DIR"),
    "/html_paragraph_element_register.rs"
));
include!(concat!(env!("OUT_DIR"), "/html_media_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_video_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_audio_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_canvas_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_meta_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_source_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_dialog_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_slot_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/shadow_root_register.rs"));
include!(concat!(
    env!("OUT_DIR"),
    "/svg_graphics_element_register.rs"
));
include!(concat!(env!("OUT_DIR"), "/svgsvg_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/svga_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_anchor_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_image_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_label_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_form_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_input_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_button_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_select_element_register.rs"));
include!(concat!(
    env!("OUT_DIR"),
    "/html_text_area_element_register.rs"
));
include!(concat!(env!("OUT_DIR"), "/html_option_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_script_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_link_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/html_style_element_register.rs"));
include!(concat!(env!("OUT_DIR"), "/dom_rect_read_only_register.rs"));
include!(concat!(env!("OUT_DIR"), "/dom_rect_register.rs"));
include!(concat!(env!("OUT_DIR"), "/dom_rect_list_register.rs"));
include!(concat!(env!("OUT_DIR"), "/style_sheet_list_register.rs"));
include!(concat!(env!("OUT_DIR"), "/node_register.rs"));
include!(concat!(env!("OUT_DIR"), "/text_register.rs"));
include!(concat!(env!("OUT_DIR"), "/cdata_section_register.rs"));
include!(concat!(env!("OUT_DIR"), "/abstract_range_register.rs"));
include!(concat!(env!("OUT_DIR"), "/range_register.rs"));
include!(concat!(env!("OUT_DIR"), "/static_range_register.rs"));
include!(concat!(env!("OUT_DIR"), "/mutation_observer_register.rs"));
include!(concat!(env!("OUT_DIR"), "/mutation_record_register.rs"));
include!(concat!(env!("OUT_DIR"), "/url_register.rs"));
include!(concat!(env!("OUT_DIR"), "/url_search_params_register.rs"));
include!(concat!(env!("OUT_DIR"), "/headers_register.rs"));
include!(concat!(env!("OUT_DIR"), "/abort_signal_register.rs"));
include!(concat!(env!("OUT_DIR"), "/abort_controller_register.rs"));
include!(concat!(env!("OUT_DIR"), "/text_decoder_register.rs"));
include!(concat!(env!("OUT_DIR"), "/text_encoder_register.rs"));
include!(concat!(env!("OUT_DIR"), "/response_register.rs"));
include!(concat!(env!("OUT_DIR"), "/request_register.rs"));
include!(concat!(
    env!("OUT_DIR"),
    "/xml_http_request_event_target_register.rs"
));
include!(concat!(env!("OUT_DIR"), "/xml_http_request_register.rs"));
include!(concat!(env!("OUT_DIR"), "/blob_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/blob_register.rs"));
include!(concat!(env!("OUT_DIR"), "/file_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/file_register.rs"));
include!(concat!(env!("OUT_DIR"), "/form_data_ops.rs"));
include!(concat!(env!("OUT_DIR"), "/form_data_register.rs"));

#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/character_data_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/event_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/custom_event_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/ui_event_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/composition_event_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/focus_event_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/keyboard_event_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/mouse_event_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/wheel_event_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/progress_event_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/htmli_frame_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/svg_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/document_fragment_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_template_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/event_target_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/comment_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/document_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/document_type_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/processing_instruction_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/dom_implementation_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/dom_parser_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/xml_serializer_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_collection_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/node_list_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/named_node_map_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/attr_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/dom_token_list_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/dom_string_map_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/style_sheet_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/css_style_sheet_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/css_rule_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/css_grouping_rule_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/css_style_rule_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/css_rule_list_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/css_style_declaration_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/css_style_properties_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/storage_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/crypto_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/css_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_div_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_span_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_paragraph_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_media_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_video_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_audio_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_canvas_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_meta_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_source_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_dialog_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_slot_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/shadow_root_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/svg_graphics_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/svgsvg_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/svga_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_anchor_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_image_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_label_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_form_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_input_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_button_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_select_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_text_area_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_option_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_script_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_link_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/html_style_element_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/dom_rect_read_only_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/dom_rect_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/dom_rect_list_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/style_sheet_list_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/node_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/text_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/cdata_section_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/abstract_range_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/range_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/static_range_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/mutation_observer_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/mutation_record_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/url_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/url_search_params_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/headers_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/abort_signal_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/abort_controller_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/text_decoder_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/text_encoder_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/response_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/request_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(
    env!("OUT_DIR"),
    "/xml_http_request_event_target_v8.rs"
));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/xml_http_request_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/blob_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/file_v8.rs"));
#[cfg(feature = "v8")]
include!(concat!(env!("OUT_DIR"), "/form_data_v8.rs"));

/// Generated QuickJS installers, separate from identically named V8 installers.
#[cfg(feature = "quickjs")]
pub mod quickjs {
    use super::*;
    use rquickjs::FromJs;
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/character_data_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/event_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/custom_event_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/ui_event_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/composition_event_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/focus_event_qjs.rs"));
    include!(concat!(env!("OUT_DIR"), "/keyboard_event_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/mouse_event_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/wheel_event_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/progress_event_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/htmli_frame_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/svg_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/document_fragment_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_template_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/event_target_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/comment_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/document_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/document_type_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/processing_instruction_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/dom_implementation_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/dom_parser_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/xml_serializer_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_collection_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/node_list_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/named_node_map_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/attr_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/dom_token_list_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/dom_string_map_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/style_sheet_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/css_style_sheet_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/css_rule_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/css_grouping_rule_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/css_style_rule_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/css_rule_list_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/css_style_declaration_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/css_style_properties_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/storage_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/crypto_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/css_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_div_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_span_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_paragraph_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_media_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_video_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_audio_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_canvas_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_meta_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_source_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_dialog_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_slot_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/shadow_root_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/svg_graphics_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/svgsvg_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/svga_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_anchor_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_image_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_label_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_form_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_input_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_button_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_select_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_text_area_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_option_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_script_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_link_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/html_style_element_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/dom_rect_read_only_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/dom_rect_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/dom_rect_list_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/style_sheet_list_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/node_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/text_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/cdata_section_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/abstract_range_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/range_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/static_range_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/mutation_observer_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/mutation_record_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/url_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/url_search_params_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/headers_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/abort_signal_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/abort_controller_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/text_decoder_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/text_encoder_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/response_qjs.rs"));
    include!(concat!(env!("OUT_DIR"), "/request_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(
        env!("OUT_DIR"),
        "/xml_http_request_event_target_qjs.rs"
    ));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/xml_http_request_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/blob_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/file_qjs.rs"));
    #[cfg(feature = "quickjs")]
    include!(concat!(env!("OUT_DIR"), "/form_data_qjs.rs"));
}

pub mod host;

#[cfg(any(feature = "v8", feature = "quickjs"))]
pub mod style_properties;

/// Runtime coverage for the mechanisms the generated pair-iterator glue rests
/// on: the two prelude helpers a `iterable<K, V>` declaration is built from,
/// and the V8 template behaviour that makes `@@iterator` identical to
/// `entries`.
///
/// The generated `entries`/`keys`/`values` callbacks need an interface with a
/// host behind it. `URLSearchParams` is one, but its callbacks run inside a page
/// engine, so these tests reach the helpers on a bare isolate where the
/// template's own behaviour can be observed directly.
#[cfg(all(test, feature = "v8"))]
mod prelude_helpers {
    use super::host::{HostContext, HostObject, realm_context};
    use super::*;

    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::dom::events::{CallbackRef, HostValue, PromiseOutcome, PromiseRef, ScriptHost};
    use crate::event_loop::{Clock, MonoTime, SharedClock};
    use tenta_dom::Document;

    /// Runs `check` on a fresh isolate with one entered context.
    fn with_context(check: impl FnOnce(&mut v8::PinScope<'_, '_>)) {
        // The V8 platform is process-global and may be initialised only once.
        static PLATFORM: std::sync::Once = std::sync::Once::new();
        PLATFORM.call_once(|| {
            let platform = v8::new_default_platform(0, false).make_shared();
            v8::V8::initialize_platform(platform);
            v8::V8::initialize();
        });

        let mut isolate = v8::Isolate::new(v8::CreateParams::default());
        v8::scope!(let handle_scope, &mut isolate);
        let context = v8::Context::new(handle_scope, Default::default());
        let mut context_scope = v8::ContextScope::new(handle_scope, context);
        v8::scope!(let scope, &mut context_scope);
        check(scope);
    }

    /// Two properties set from one `FunctionTemplate` hold *one* function
    /// object, which is how the generated pair iterator makes `@@iterator` the
    /// same function object as `entries` without giving up the template API for
    /// the other three members.
    ///
    /// WebIDL asks for that identity, and `docs/dev/idl.md` records it as
    /// satisfied. If a future V8 stops instantiating a template once per
    /// context, that record is what has to change, so this asserts both halves:
    /// one template is one function, two templates are two.
    #[test]
    fn one_template_is_one_function_object_per_context() {
        with_context(|scope| {
            let prototype = v8::ObjectTemplate::new(scope);
            let shared = v8::FunctionTemplate::builder(
                |_: &mut v8::PinScope<'_, '_>,
                 _: v8::FunctionCallbackArguments<'_>,
                 _: v8::ReturnValue<v8::Value>| {},
            )
            .length(0)
            .build(scope);
            let other = v8::FunctionTemplate::builder(
                |_: &mut v8::PinScope<'_, '_>,
                 _: v8::FunctionCallbackArguments<'_>,
                 _: v8::ReturnValue<v8::Value>| {},
            )
            .length(1)
            .build(scope);
            for (name, template) in [("entries", shared), ("keys", shared), ("values", other)] {
                let key = v8_string(scope, name).expect("the name fits a V8 string");
                prototype.set_with_attr(key.into(), template.into(), v8::PropertyAttribute::NONE);
            }
            let iterator = v8::Symbol::get_iterator(scope);
            prototype.set_with_attr(
                iterator.into(),
                shared.into(),
                v8::PropertyAttribute::DONT_ENUM,
            );

            let instance = prototype
                .new_instance(scope)
                .expect("the template instantiates");
            let entries_key = v8_string(scope, "entries").expect("a V8 string");
            let keys_key = v8_string(scope, "keys").expect("a V8 string");
            let values_key = v8_string(scope, "values").expect("a V8 string");
            let entries = instance.get(scope, entries_key.into()).expect("set");
            let keys = instance.get(scope, keys_key.into()).expect("set");
            let values = instance.get(scope, values_key.into()).expect("set");
            let symbol = instance.get(scope, iterator.into()).expect("set");

            assert!(entries.strict_equals(keys), "one template, one function");
            assert!(
                entries.strict_equals(symbol),
                "`@@iterator` is `entries`, not a copy of it"
            );
            assert!(
                !entries.strict_equals(values),
                "a second template is a second function, so the asserts above are not vacuous"
            );
        });
    }

    /// Evaluates `source` in the entered context and returns its result as a
    /// string.
    fn eval(scope: &mut v8::PinScope<'_, '_>, source: &str) -> String {
        let source = v8_string(scope, source).expect("the source fits a V8 string");
        let script = v8::Script::compile(scope, source, None).expect("the source compiles");
        script
            .run(scope)
            .expect("the source runs without throwing")
            .to_rust_string_lossy(scope)
    }

    /// Reads a global by name.
    fn global<'s>(scope: &mut v8::PinScope<'s, '_>, name: &str) -> v8::Local<'s, v8::Value> {
        let key = v8_string(scope, name).expect("the name fits a V8 string");
        scope
            .get_current_context()
            .global(scope)
            .get(scope, key.into())
            .unwrap_or_else(|| panic!("`{name}` is a global"))
    }

    /// Reads a global that holds a function.
    fn global_function<'s>(
        scope: &mut v8::PinScope<'s, '_>,
        name: &str,
    ) -> v8::Local<'s, v8::Function> {
        v8::Local::<v8::Function>::try_from(global(scope, name))
            .unwrap_or_else(|_| panic!("`{name}` is a function"))
    }

    #[test]
    fn array_iterator_yields_the_arrays_values() {
        with_context(|scope| {
            let mut items: Vec<v8::Local<v8::Value>> = Vec::new();
            for item in ["a", "b", "c"] {
                items.push(
                    v8_string(scope, item)
                        .expect("the item fits a V8 string")
                        .into(),
                );
            }
            let array = v8::Array::new_with_elements(scope, &items);
            let iterator = array_iterator(scope, array).expect("the iterator is built");

            // `%Array.prototype.values%` is cached in an isolate slot the first
            // time, so a second call exercises the cached path.
            let empty = v8::Array::new(scope, 0);
            let cached = array_iterator(scope, empty).expect("the cached intrinsic still works");

            let global = scope.get_current_context().global(scope);
            for (name, value) in [("__iterator", iterator), ("__cached", cached)] {
                let key = v8_string(scope, name).expect("the name fits a V8 string");
                global.set(scope, key.into(), value);
            }

            // An Array Iterator, not an interface-branded one.
            assert_eq!(
                eval(
                    scope,
                    "{
                        const seen = [...__iterator];
                        [Object.prototype.toString.call(__iterator),
                         seen.join(','), seen.length].join('|');
                    }"
                ),
                "[object Array Iterator]|a,b,c|3"
            );
            assert_eq!(
                eval(scope, "[...__cached].length.toString()"),
                "0",
                "an empty array still yields an Array Iterator"
            );
        });
    }

    #[test]
    fn call_js_function_passes_the_receiver_as_this() {
        with_context(|scope| {
            eval(
                scope,
                "globalThis.__holder = { label: 'ctx', read: function () { return this.label; } };",
            );
            let holder = global(scope, "__holder");
            let key = v8_string(scope, "read").expect("the key fits a V8 string");
            let read = v8::Local::<v8::Object>::try_from(holder)
                .ok()
                .and_then(|object| object.get(scope, key.into()))
                .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
                .expect("`read` is a function");

            let value = call_js_function(scope, read, holder, &[]).expect("the call succeeds");
            assert_eq!(value.to_rust_string_lossy(scope), "ctx");
        });
    }

    /// The helper must rethrow, not merely report: an exception that reaches
    /// only the returned `JsError` would be lost to the page. Relying on the
    /// `TryCatch` destructor would swallow it instead, which is what the
    /// event-listener path in `tenta-js-v8` wants and therefore what this
    /// helper may not depend on.
    #[test]
    fn call_js_function_rethrows_into_the_enclosing_try_catch() {
        with_context(|scope| {
            eval(
                scope,
                "globalThis.__boom = function () { throw new RangeError('boom'); };",
            );
            let boom = global_function(scope, "__boom");

            v8::tc_scope!(let tc, scope);
            let receiver = v8::undefined(tc).into();
            let error =
                call_js_function(tc, boom, receiver, &[]).expect_err("the page function throws");
            assert_eq!(error.kind(), JsErrorKind::Type);
            assert!(
                tc.has_caught(),
                "the exception never reached the enclosing `TryCatch`"
            );
            let thrown = tc.exception().expect("the original exception is retained");
            let message = thrown
                .to_string(tc)
                .expect("RangeError can be stringified")
                .to_rust_string_lossy(tc);
            assert!(
                message.contains("RangeError: boom"),
                "unexpected exception: {message}"
            );
        });
    }

    /// WebIDL converts a sequence with `GetMethod(V, @@iterator)` followed by
    /// `GetIterator`, so anything iterable is a sequence and not only an array.
    /// `new URLSearchParams(new Map([["a", "b"]]))` is the caller that needs
    /// this; an array takes the same path, with no special case to get wrong.
    #[test]
    fn a_sequence_is_read_through_the_iterator_protocol() {
        with_context(|scope| {
            eval(scope, "globalThis.__set = new Set(['a', 'b']);");
            let set = global(scope, "__set");
            let items = sequence_from_v8(scope, set, string_from_v8)
                .expect("a `Set` is iterable, so it is a sequence");
            assert_eq!(items, ["a", "b"]);

            eval(scope, "globalThis.__items = ['x'];");
            let array = global(scope, "__items");
            let items =
                sequence_from_v8(scope, array, string_from_v8).expect("an array is a sequence");
            assert_eq!(items, ["x"]);
        });
    }

    /// A value without a usable `@@iterator` is the `TypeError` the spec
    /// prescribes, including a primitive, which is not an object at all.
    #[test]
    fn a_value_without_an_iterator_is_not_a_sequence() {
        with_context(|scope| {
            eval(scope, "globalThis.__plain = { a: '1' };");
            let plain = global(scope, "__plain");
            let error = sequence_from_v8(scope, plain, string_from_v8)
                .expect_err("an object without `@@iterator` is not a sequence");
            assert_eq!(error.kind(), JsErrorKind::Type);

            // A primitive string *is* iterable in JavaScript, but WebIDL's
            // conversion rejects a non-object before it ever looks for the
            // method, so this is a `TypeError` too.
            let value = v8_string(scope, "abc")
                .expect("the text fits a V8 string")
                .into();
            let error = sequence_from_v8(scope, value, string_from_v8)
                .expect_err("a primitive is not a sequence");
            assert_eq!(error.kind(), JsErrorKind::Type);
        });
    }

    /// `es-to-record` filters on the property descriptor: an own property that
    /// is not enumerable never reaches the record, an accessor is read rather
    /// than treated as data, and an inherited enumerable property is not an own
    /// key at all. `headers-record.any.js` observes the descriptor half.
    ///
    /// The integer-like key is the one that catches the descriptor read out: a
    /// key reported as a Number has no `Name` form, so an entry that is not
    /// converted to its string name is dropped instead of read.
    #[test]
    fn a_record_reads_only_the_enumerable_own_properties() {
        with_context(|scope| {
            eval(
                scope,
                "globalThis.__source = { visible: 'v' };
                 Object.defineProperty(__source, 'hidden', { value: 'h', enumerable: false });
                 Object.defineProperty(__source, 'computed', { get: () => 'g', enumerable: true });
                 globalThis.__numbered = { 0: 'zero', a: 'a' };
                 globalThis.__inherited = Object.create({ inherited: 'i' });
                 __inherited.own = 'o';",
            );
            let source = global(scope, "__source");
            let entries = record_from_v8(scope, source, string_from_v8, string_from_v8)
                .expect("the source is a record");
            assert_eq!(
                entries,
                [
                    ("visible".to_string(), "v".to_string()),
                    ("computed".to_string(), "g".to_string())
                ],
                "the non-enumerable own property leaked into the record"
            );

            let source = global(scope, "__numbered");
            let entries = record_from_v8(scope, source, string_from_v8, string_from_v8)
                .expect("the source is a record");
            assert_eq!(
                entries,
                [
                    ("0".to_string(), "zero".to_string()),
                    ("a".to_string(), "a".to_string())
                ],
                "an integer-like own key was dropped"
            );

            let source = global(scope, "__inherited");
            let entries = record_from_v8(scope, source, string_from_v8, string_from_v8)
                .expect("the source is a record");
            assert_eq!(
                entries,
                [("own".to_string(), "o".to_string())],
                "an inherited enumerable property leaked into the record"
            );
        });
    }

    /// A `ByteString` is one code unit per byte: `0xFF` survives the round
    /// trip, where a UTF-8 conversion would have rewritten it, and a code unit
    /// above `0xFF` is the `TypeError` WebIDL prescribes rather than a
    /// truncation to its low byte.
    #[test]
    fn a_byte_string_round_trips_isomorphically() {
        with_context(|scope| {
            eval(scope, "globalThis.__byte = '\\u00ff';");
            let value = global(scope, "__byte");
            let bytes = byte_string_from_v8(scope, value).expect("`0xFF` is a valid `ByteString`");
            assert_eq!(bytes, [0xFF]);

            let back = byte_string_to_v8(scope, &bytes).expect("the bytes fit a V8 string");
            assert_eq!(back.to_rust_string_lossy(scope), "\u{FF}");

            eval(scope, "globalThis.__too_big = '\\u0100';");
            let value = global(scope, "__too_big");
            let error = byte_string_from_v8(scope, value)
                .expect_err("a code unit above `0xFF` is not a `ByteString`");
            assert_eq!(error.kind(), JsErrorKind::Type);
        });
    }

    /// An abrupt `[[GetOwnProperty]]` fails the conversion instead of dropping
    /// the key.
    ///
    /// `es-to-record` reads each own key's descriptor with `?`, so a proxy whose
    /// trap throws has to fail the whole conversion. Treating the unreadable
    /// descriptor as "not enumerable" would return an *empty* record as if the
    /// object had had no enumerable properties, which is the wrong answer this
    /// asserts against — the loop would otherwise `continue` past every key and
    /// succeed.
    #[test]
    fn a_record_propagates_a_failing_descriptor_read() {
        with_context(|scope| {
            eval(
                scope,
                "globalThis.__trap = new Proxy({ a: '1' }, {
                     getOwnPropertyDescriptor() { throw new Error('no descriptors'); },
                   });",
            );
            let source = global(scope, "__trap");
            let error = record_from_v8(scope, source, string_from_v8, string_from_v8)
                .expect_err("an abrupt descriptor read fails the conversion");
            assert_eq!(error.kind(), JsErrorKind::Type);
        });
    }

    /// `es-to-record` applies the *declared* key type's conversion to each own
    /// enumerable key, so a `ByteString`-keyed record reads every key as one
    /// code unit per byte: `'é'` (U+00E9) is the single byte `0xE9`, and a code
    /// unit above `0xFF` — `'€'` (U+20AC) — is the `TypeError` WebIDL
    /// prescribes rather than a truncation to its low byte.
    ///
    /// The enumeration rules are key-independent, so the `ByteString` key
    /// conversion has to skip a non-enumerable own property and never see an
    /// inherited one, exactly as the string-keyed record does.
    #[test]
    fn a_record_reads_its_keys_through_the_declared_key_type() {
        with_context(|scope| {
            eval(
                scope,
                "globalThis.__bytes = { '\\u00e9': 'v' };
                 Object.defineProperty(__bytes, 'hidden', { value: 'h', enumerable: false });
                 globalThis.__inherited_bytes = Object.create({ inherited: 'i' });
                 __inherited_bytes.own = 'o';
                 globalThis.__wide = { '\\u20ac': 'v' };",
            );

            let source = global(scope, "__bytes");
            let entries = record_from_v8(scope, source, byte_string_from_v8, |scope, item| {
                string_from_v8(scope, item)
            })
            .expect("a Latin-1 key is a valid `ByteString`");
            assert_eq!(
                entries,
                [(vec![0xE9], "v".to_string())],
                "the U+00E9 key is not its single byte `0xE9`"
            );

            let source = global(scope, "__inherited_bytes");
            let entries = record_from_v8(scope, source, byte_string_from_v8, |scope, item| {
                string_from_v8(scope, item)
            })
            .expect("the own key is a valid `ByteString`");
            assert_eq!(
                entries,
                [(b"own".to_vec(), "o".to_string())],
                "an inherited enumerable property leaked into a `ByteString`-keyed record"
            );

            let source = global(scope, "__wide");
            let error = record_from_v8(scope, source, byte_string_from_v8, |scope, item| {
                string_from_v8(scope, item)
            })
            .expect_err("a code unit above `0xFF` is not a `ByteString` key");
            assert_eq!(error.kind(), JsErrorKind::Type);
        });
    }

    /// The write direction keys the same way: a `ByteString` key of `0xE9`
    /// reaches script as the one-code-unit name `'é'`, where a `USVString` key
    /// of `'\u{FF}'` reaches it unchanged. The two key conversions are what
    /// `record_to_v8` is asked for, and neither is the other.
    #[test]
    fn a_record_writes_its_keys_through_the_declared_key_type() {
        with_context(|scope| {
            let bytes = record_to_v8(
                scope,
                vec![(vec![0xE9u8], "v".to_string())],
                |scope, key| byte_string_to_v8(scope, &key),
                |scope, item| string_to_v8(scope, &item),
            )
            .expect("the byte fits a V8 string");
            let text = record_to_v8(
                scope,
                vec![("\u{FF}".to_string(), "v".to_string())],
                |scope, key| string_to_v8(scope, &key),
                |scope, item| string_to_v8(scope, &item),
            )
            .expect("the string fits a V8 string");
            let global_object = scope.get_current_context().global(scope);
            for (name, object) in [("__bytes", bytes), ("__text", text)] {
                let key = v8_string(scope, name).expect("the name fits a V8 string");
                global_object.set(scope, key.into(), object);
            }

            assert_eq!(
                eval(
                    scope,
                    "{ const keys = Object.keys(__bytes);
                       [keys.length, keys[0].charCodeAt(0), __bytes[keys[0]]].join('|'); }"
                ),
                "1|233|v",
                "the byte key did not reach script as the one-code-unit name"
            );
            assert_eq!(
                eval(
                    scope,
                    "{ const keys = Object.keys(__text);
                       [keys.length, keys[0].charCodeAt(0)].join('|'); }"
                ),
                "1|255",
                "a string key was rewritten on the way out"
            );
        });
    }

    /// An abrupt `done` read fails the conversion instead of ending the walk.
    ///
    /// `IteratorComplete` is `ToBoolean(Get(result, "done"))`, and an abrupt
    /// `Get` propagates. Reading it as "done" would end the iteration at the
    /// first entry and return the entries collected so far as if the iterator
    /// had finished, so a one-entry iterator whose `done` getter throws must be
    /// an error and not a one-entry sequence.
    #[test]
    fn a_sequence_propagates_a_failing_done_read() {
        with_context(|scope| {
            eval(
                scope,
                "globalThis.__iter = {
                     [Symbol.iterator]() {
                       return {
                         next() {
                           return { value: 'a', get done() { throw new Error('no done'); } };
                         },
                       };
                     },
                   };",
            );
            let source = global(scope, "__iter");
            let error = sequence_from_v8(scope, source, string_from_v8)
                .expect_err("an abrupt `done` read fails the conversion");
            assert_eq!(error.kind(), JsErrorKind::Type);
        });
    }

    // ------------------------------------------------------------------------
    // Buffer sources: `ArrayBuffer`, the typed arrays, `DataView`, and the
    // `ArrayBufferView`/`BufferSource` typedefs.
    //
    // The tests above reach the helpers with values built in Rust. These go
    // through script, because the bytes on both sides of the boundary are the
    // subject: the array the page builds is what the host has to receive, and
    // the array the host hands back has to be a native V8 typed array — one
    // with a real `length`, indexed elements and a `buffer` — and not a wrapper
    // object that merely carries the interface's name.
    // ------------------------------------------------------------------------

    /// A view is read as the range it covers, never as the buffer behind it.
    ///
    /// `new Uint8Array(store.buffer, 4, 3)` is the case the conversion exists
    /// for: a `byteOffset` that is not zero and a `byteLength` that is not the
    /// whole store. The three readers that admit a view all have to agree.
    #[test]
    fn a_view_is_read_as_the_bytes_it_covers() {
        with_context(|scope| {
            eval(
                scope,
                "const store = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]);
                 globalThis.__sub = new Uint8Array(store.buffer, 4, 3);
                 globalThis.__whole = store;
                 globalThis.__store = store.buffer;
                 globalThis.__empty = new Uint8Array(0);
                 globalThis.__zero = new ArrayBuffer(0);",
            );

            let sub = global(scope, "__sub");
            assert_eq!(
                uint8_array_from_v8(scope, sub).expect("a `Uint8Array` is one"),
                [5, 6, 7],
                "the view's own range is not what was read"
            );
            assert_eq!(
                buffer_source_from_v8(scope, sub).expect("a view is a `BufferSource`"),
                [5, 6, 7],
                "`BufferSource` read past the view"
            );
            assert_eq!(
                array_buffer_view_from_v8(scope, sub).expect("a view is an `ArrayBufferView`"),
                [5, 6, 7],
                "`ArrayBufferView` read past the view"
            );

            let whole = global(scope, "__whole");
            assert_eq!(
                uint8_array_from_v8(scope, whole).expect("a `Uint8Array` is one"),
                [1, 2, 3, 4, 5, 6, 7, 8],
                "a view that covers its whole store was truncated"
            );

            let store = global(scope, "__store");
            assert_eq!(
                array_buffer_from_v8(scope, store).expect("an `ArrayBuffer` is one"),
                [1, 2, 3, 4, 5, 6, 7, 8]
            );
            assert_eq!(
                buffer_source_from_v8(scope, store).expect("an `ArrayBuffer` is a `BufferSource`"),
                [1, 2, 3, 4, 5, 6, 7, 8]
            );

            // An empty buffer is an empty sequence, not an error.
            let zero = global(scope, "__zero");
            assert_eq!(
                array_buffer_from_v8(scope, zero).expect("an empty `ArrayBuffer` is one"),
                Vec::<u8>::new()
            );
            let empty = global(scope, "__empty");
            assert_eq!(
                uint8_array_from_v8(scope, empty).expect("an empty `Uint8Array` is one"),
                Vec::<u8>::new()
            );
        });
    }

    /// A detached buffer and every view of it hold no bytes.
    ///
    /// WebIDL's `GetACopyOfTheBufferSource` reads a detached buffer as the empty
    /// byte sequence rather than failing, which is what
    /// `encoding/textdecoder-arguments.any.js` observes of the page-visible
    /// behaviour of these conversions. `ArrayBuffer.prototype.transfer()`
    /// detaches the original and leaves the view over it with a zero length.
    #[test]
    fn a_detached_buffer_reads_as_no_bytes() {
        with_context(|scope| {
            eval(
                scope,
                "globalThis.__detached = new ArrayBuffer(4);
                 globalThis.__over = new Uint8Array(__detached);
                 __over.set([1, 2, 3, 4]);
                 __detached.transfer();",
            );

            let over = global(scope, "__over");
            assert_eq!(
                eval(scope, "__over.length.toString()"),
                "0",
                "the buffer was not detached, so the rest of this test proves nothing"
            );
            assert!(
                uint8_array_from_v8(scope, over)
                    .expect("a view over a detached buffer is still a `Uint8Array`")
                    .is_empty(),
                "a detached view read as non-empty"
            );

            let detached = global(scope, "__detached");
            assert!(
                array_buffer_from_v8(scope, detached)
                    .expect("a detached `ArrayBuffer` is still one")
                    .is_empty(),
                "a detached `ArrayBuffer` read as non-empty"
            );
            assert!(
                buffer_source_from_v8(scope, detached)
                    .expect("a detached `ArrayBuffer` is still a `BufferSource`")
                    .is_empty()
            );
        });
    }

    /// Bytes written to script arrive as a native typed array.
    ///
    /// `instanceof`, `length`, `byteLength`, indexed access and `.buffer` are
    /// all observed from JavaScript, because a host-object wrapper carrying the
    /// interface's name would satisfy none of them — which is exactly how the
    /// previous lowering failed at runtime.
    #[test]
    fn bytes_written_to_script_are_a_real_typed_array() {
        with_context(|scope| {
            let mut numbers = Vec::new();
            numbers.extend_from_slice(&1.5f64.to_ne_bytes());
            numbers.extend_from_slice(&(-2.25f64).to_ne_bytes());

            for (name, value) in [
                (
                    "__bytes",
                    uint8_array_to_v8(scope, vec![1, 2, 3]).expect("the bytes fit"),
                ),
                (
                    "__numbers",
                    float64_array_to_v8(scope, numbers).expect("the bytes fit"),
                ),
                (
                    "__source",
                    buffer_source_to_v8(scope, vec![6]).expect("the bytes fit"),
                ),
                (
                    "__any_view",
                    array_buffer_view_to_v8(scope, vec![5]).expect("the bytes fit"),
                ),
                (
                    "__buffer",
                    array_buffer_to_v8(scope, vec![7, 8]).expect("the bytes fit"),
                ),
                (
                    "__data_view",
                    data_view_to_v8(scope, vec![9, 10]).expect("the bytes fit"),
                ),
            ] {
                let key = v8_string(scope, name).expect("the name fits a V8 string");
                scope
                    .get_current_context()
                    .global(scope)
                    .set(scope, key.into(), value);
            }

            assert_eq!(
                eval(
                    scope,
                    "[__bytes instanceof Uint8Array,
                      __bytes instanceof Object,
                      ArrayBuffer.isView(__bytes),
                      __bytes.length, __bytes.byteLength,
                      __bytes[0], __bytes[2], __bytes.buffer.byteLength,
                      Object.prototype.toString.call(__bytes)].join('|')"
                ),
                "true|true|true|3|3|1|3|3|[object Uint8Array]",
                "the value is not a native `Uint8Array`"
            );
            assert_eq!(
                eval(
                    scope,
                    "[__numbers instanceof Float64Array,
                      __numbers.length, __numbers.byteLength,
                      __numbers[0], __numbers[1],
                      Object.prototype.toString.call(__numbers)].join('|')"
                ),
                "true|2|16|1.5|-2.25|[object Float64Array]",
                "the bytes did not land on whole 8-byte elements"
            );
            assert_eq!(
                eval(
                    scope,
                    "[__data_view instanceof DataView,
                      __data_view instanceof Uint8Array,
                      __data_view.byteOffset, __data_view.byteLength,
                      __data_view.getUint8(0), __data_view.getUint8(1),
                      Object.prototype.toString.call(__data_view)].join('|')"
                ),
                "true|false|0|2|9|10|[object DataView]",
                "the value is not a native `DataView`"
            );
            assert_eq!(
                eval(
                    scope,
                    "[Object.prototype.toString.call(__buffer),
                      __buffer.byteLength,
                      new Uint8Array(__buffer)[0], new Uint8Array(__buffer)[1]].join('|')"
                ),
                "[object ArrayBuffer]|2|7|8",
                "the value is not a native `ArrayBuffer`"
            );
            // `BufferSource` and `ArrayBufferView` are typedefs: the value is
            // one of their members, not an object of a type of its own.
            assert_eq!(
                eval(
                    scope,
                    "[__source instanceof Uint8Array, __source.length,
                      __any_view instanceof Uint8Array, __any_view.length].join('|')"
                ),
                "true|1|true|1"
            );
        });
    }

    /// A byte count that is not a whole number of elements is a `RangeError`
    /// rather than a silently truncated array.
    #[test]
    fn a_buffer_write_that_is_not_whole_elements_is_a_range_error() {
        with_context(|scope| {
            let error = float64_array_to_v8(scope, vec![0; 6])
                .expect_err("six bytes are not whole `Float64Array` elements");
            assert_eq!(error.kind(), JsErrorKind::Range);
            assert!(
                error.message().contains("multiple of 8"),
                "unexpected message: {}",
                error.message()
            );
        });
    }

    /// The brand check is per type, and only the types the conversion names are
    /// accepted.
    ///
    /// A `Uint8ClampedArray` is a view but not a `Uint8Array`; a `Float64Array`
    /// is a view but not a `DataView`; a `SharedArrayBuffer` is a buffer but not
    /// an `ArrayBuffer` (no declaration in the corpus carries `[AllowShared]`);
    /// and an `ArrayBuffer` is not a view at all.
    #[test]
    fn a_value_that_is_not_the_named_buffer_type_is_a_type_error() {
        with_context(|scope| {
            eval(
                scope,
                "globalThis.__clamped = new Uint8ClampedArray(1);
                 globalThis.__numbers = new Float64Array(1);
                 globalThis.__shared = new SharedArrayBuffer(4);
                 globalThis.__buffer = new ArrayBuffer(4);
                 globalThis.__plain = { length: 1, 0: 1 };
                 globalThis.__text = 'abc';
                 globalThis.__bigints = new BigInt64Array(1);",
            );

            for (name, message) in [("__plain", "a plain object"), ("__text", "a string")] {
                let value = global(scope, name);
                assert_eq!(
                    buffer_source_from_v8(scope, value)
                        .expect_err("not a buffer source")
                        .kind(),
                    JsErrorKind::Type,
                    "{message} was accepted as a `BufferSource`"
                );
                assert!(array_buffer_from_v8(scope, value).is_err());
                assert!(array_buffer_view_from_v8(scope, value).is_err());
            }

            let clamped = global(scope, "__clamped");
            assert_eq!(
                uint8_array_from_v8(scope, clamped)
                    .expect_err("a `Uint8ClampedArray` is not a `Uint8Array`")
                    .kind(),
                JsErrorKind::Type,
                "the brand check admitted the wrong view type"
            );
            assert!(
                buffer_source_from_v8(scope, clamped).is_ok(),
                "a `Uint8ClampedArray` is a `BufferSource`"
            );

            let numbers = global(scope, "__numbers");
            assert!(data_view_from_v8(scope, numbers).is_err());
            assert_eq!(
                uint8_array_from_v8(scope, numbers)
                    .expect_err("a `Float64Array` is not a `Uint8Array`")
                    .kind(),
                JsErrorKind::Type
            );

            let shared = global(scope, "__shared");
            assert!(array_buffer_from_v8(scope, shared).is_err());
            assert!(buffer_source_from_v8(scope, shared).is_err());

            let buffer = global(scope, "__buffer");
            assert!(
                array_buffer_view_from_v8(scope, buffer).is_err(),
                "an `ArrayBuffer` is not an `ArrayBufferView`"
            );
            assert!(data_view_from_v8(scope, buffer).is_err());
            assert!(uint8_array_from_v8(scope, buffer).is_err());
            assert!(array_buffer_from_v8(scope, buffer).is_ok());
            assert!(buffer_source_from_v8(scope, buffer).is_ok());

            // Nothing here brands on the element type, so an array whose name
            // the generator cannot spell is still a view. The two `BigInt`
            // array spellings have no weedle keyword arm, which costs a
            // *declaration* the ability to name one, not a value the ability to
            // be converted.
            let bigints = global(scope, "__bigints");
            assert_eq!(
                buffer_source_from_v8(scope, bigints)
                    .expect("a `BigInt64Array` is an `ArrayBufferView`")
                    .len(),
                8,
                "the eight bytes of one `BigInt64` element were not read"
            );
            assert!(
                array_buffer_view_from_v8(scope, bigints).is_ok(),
                "a `BigInt64Array` is an `ArrayBufferView`"
            );
        });
    }

    // -----------------------------------------------------------------------
    // `TextDecoder` and `TextEncoder`
    // -----------------------------------------------------------------------

    /// A [`Realm`] over a fresh [`HostContext`], which is all a generated
    /// constructor needs to place a record and all a generated member needs to
    /// reach one.
    ///
    /// The two encoding hosts are the first whose *algorithms* are wholly the
    /// host's: a `TextDecoder` owns a decoder that survives from one
    /// `{stream: true}` call to the next, and a BOM rule of its own that no
    /// generated glue knows about. Both are observable without a wrapper world,
    /// so they are tested here; what does need the world — the argument
    /// conversions, `instanceof`, and the typed array `encode` hands back — is
    /// tested in `crates/tenta-js-v8/tests/dom_encoding.rs`.
    fn host_realm() -> Realm {
        let document = Rc::new(RefCell::new(Document::new()));
        let clock = SharedClock::new(Clock::virtual_at(MonoTime::ZERO));
        let context: Realm = HostContext::new(
            document,
            clock,
            tenta_net::CookieJar::new(),
            crate::storage::PageStorage::isolated(),
        );
        context
    }

    /// A `TextDecoder` over `label`, with both options spelled out.
    fn decoder(realm: &Realm, label: Option<&str>, fatal: bool, ignore_bom: bool) -> HostObject {
        <HostObject as TextDecoderConstructor>::construct(
            realm,
            label,
            Some(TextDecoderOptions { fatal, ignore_bom }),
        )
        .expect("the label names an encoding")
    }

    /// The `decode(input)` shape: no options, so not a streaming call.
    fn decode(host: &HostObject, bytes: &[u8]) -> String {
        <HostObject as TextDecoderOps>::decode(host, Some(bytes.to_vec()), None)
            .expect("valid input decodes without a `fatal` error")
    }

    /// The `decode(input, options)` shape.
    fn decode_with(host: &HostObject, bytes: &[u8], stream: bool) -> Result<String, JsError> {
        <HostObject as TextDecoderOps>::decode(
            host,
            Some(bytes.to_vec()),
            Some(TextDecodeOptions { stream }),
        )
    }

    /// A label is never kept: what the constructor resolves it to is an
    /// encoding, and the name it reports is the standard's own, ASCII
    /// lowercased. Whitespace around a label is part of the lookup.
    #[test]
    fn text_decoder_labels_resolve_to_an_encoding_name() {
        let realm = host_realm();
        for (label, name) in [
            (None, "utf-8"),
            (Some("utf-8"), "utf-8"),
            (Some("UTF-8"), "utf-8"),
            (Some("  utf-8  "), "utf-8"),
            (Some("utf8"), "utf-8"),
            (Some("utf-16le"), "utf-16le"),
            (Some("latin1"), "windows-1252"),
            (Some("windows-1252"), "windows-1252"),
            (Some("iso-8859-2"), "iso-8859-2"),
            (Some("shift_jis"), "shift_jis"),
        ] {
            let host = decoder(&realm, label, false, false);
            assert_eq!(
                <HostObject as TextDecoderOps>::encoding(&host).expect("an encoding has a name"),
                name,
                "label {label:?}"
            );
            assert!(!host.fatal(), "`fatal` defaults to false for {label:?}");
            assert!(
                !host.ignore_bom(),
                "`ignoreBOM` defaults to false for {label:?}"
            );
        }
    }

    /// The two options are the constructor's, not the encoding's.
    #[test]
    fn text_decoder_reads_back_both_options() {
        let realm = host_realm();
        let host = decoder(&realm, Some("utf-16be"), true, true);
        assert!(host.fatal());
        assert!(host.ignore_bom());
        assert_eq!(
            <HostObject as TextDecoderOps>::encoding(&host).expect("an encoding has a name"),
            "utf-16be"
        );
    }

    /// A label that names no encoding is a `RangeError`. The empty label is
    /// one of them: no encoding has the empty string as a label.
    #[test]
    fn text_decoder_rejects_labels_that_are_not_encodings() {
        let realm = host_realm();
        for label in ["no-such-encoding", "", "utf-9", "utf-8x"] {
            let error =
                <HostObject as TextDecoderConstructor>::construct(&realm, Some(label), None)
                    .err()
                    .expect("an unknown label is refused");
            assert_eq!(error.kind(), JsErrorKind::Range, "label {label:?}");
        }
    }

    /// The Encoding Standard's *replacement* encodings are the labels that
    /// resolve to a decoder which swallows its whole input and emits one
    /// replacement character. `for_label_no_replacement` — which is why the
    /// host calls that and not `for_label` — drops them from the table, so the
    /// constructor refuses them like any other unknown label. This is the whole
    /// of `api-replacement-encodings.any.js`.
    #[test]
    fn text_decoder_rejects_the_replacement_encodings() {
        let realm = host_realm();
        for label in [
            "replacement",
            "csiso2022kr",
            "hz-gb-2312",
            "iso-2022-cn",
            "iso-2022-cn-ext",
            "iso-2022-kr",
        ] {
            let error =
                <HostObject as TextDecoderConstructor>::construct(&realm, Some(label), None)
                    .err()
                    .expect("a replacement encoding is refused");
            assert_eq!(error.kind(), JsErrorKind::Range, "label {label:?}");
        }
    }

    /// The *serialize I/O queue* BOM rule. The decoder is asked not to handle
    /// the BOM itself (`new_decoder_without_bom_handling`), so the U+FEFF is in
    /// the output; the host removes one leading U+FEFF after the first decode
    /// of each decoder, and only there.
    #[test]
    fn text_decoder_drops_one_leading_bom() {
        let realm = host_realm();
        let host = decoder(&realm, None, false, false);

        assert_eq!(
            decode(&host, &[]),
            "",
            "the empty input is the empty string"
        );
        assert_eq!(decode(&host, &[0xEF, 0xBB, 0xBF, 0x41]), "A");
        assert_eq!(
            decode(&host, &[0xEF, 0xBB, 0xBF, 0xEF, 0xBB, 0xBF, 0x41]),
            "\u{FEFF}A",
            "only the first of two BOMs goes"
        );
        assert_eq!(
            decode(&host, &[0x41, 0xEF, 0xBB, 0xBF]),
            "A\u{FEFF}",
            "a BOM that is not leading is text"
        );
        assert_eq!(
            decode(&host, &[0xEF, 0xBB, 0xBF, 0x42]),
            "B",
            "a non-streaming call replaces the decoder, so the rule applies again"
        );
    }

    /// `ignoreBOM` keeps the character the rule would have removed.
    #[test]
    fn text_decoder_keeps_the_bom_when_asked_to_ignore_it() {
        let realm = host_realm();
        let host = decoder(&realm, None, false, true);
        assert_eq!(decode(&host, &[0xEF, 0xBB, 0xBF, 0x41]), "\u{FEFF}A");
        assert_eq!(
            decode(&host, &[0xEF, 0xBB, 0xBF, 0x42]),
            "\u{FEFF}B",
            "and it keeps keeping it, for every decoder"
        );
    }

    /// The rule is the standard's, not UTF-8's: it applies only to the
    /// encodings whose BOM is a byte-order mark. Windows-1252 has no BOM, so
    /// the same three bytes are three of its own characters and stay.
    #[test]
    fn text_decoder_applies_the_bom_rule_only_to_bom_encodings() {
        let realm = host_realm();
        let bytes = [0xEF, 0xBB, 0xBF];
        assert_eq!(
            decode(&decoder(&realm, Some("windows-1252"), false, false), &bytes),
            "\u{EF}\u{BB}\u{BF}"
        );
        assert_eq!(
            decode(&decoder(&realm, Some("iso-8859-2"), false, false), &bytes),
            "\u{10F}\u{165}\u{17C}",
            "a second single-byte encoding reads the same bytes its own way"
        );
        assert_eq!(
            decode(
                &decoder(&realm, Some("utf-16le"), false, false),
                &[0xFF, 0xFE, 0x41, 0x00]
            ),
            "A",
            "a UTF-16LE byte-order mark is one"
        );
        assert_eq!(
            decode(
                &decoder(&realm, Some("utf-16be"), false, false),
                &[0xFE, 0xFF, 0x00, 0x41]
            ),
            "A",
            "and a UTF-16BE byte-order mark is the other"
        );
        assert_eq!(
            decode(
                &decoder(&realm, Some("utf-16le"), false, true),
                &[0xFF, 0xFE, 0x41, 0x00]
            ),
            "\u{FEFF}A",
            "`ignoreBOM` reaches the UTF-16 decoders too"
        );
    }

    /// `stream: true` keeps the decoder, and with it a partial sequence; the
    /// next call completes it. A call that is not streaming replaces the
    /// decoder, so whatever it held is gone.
    #[test]
    fn text_decoder_carries_a_partial_sequence_across_streaming_calls() {
        let realm = host_realm();
        let host = decoder(&realm, None, false, false);

        assert_eq!(decode_with(&host, &[0xC9], true).expect("no error"), "");
        assert_eq!(
            decode_with(&host, &[0xA9], true).expect("no error"),
            "\u{269}",
            "0xA9 completed the two-byte sequence 0xC9 0xA9"
        );
        assert_eq!(
            decode_with(&host, &[], false).expect("no error"),
            "",
            "nothing is held once the sequence is complete"
        );

        assert_eq!(decode_with(&host, &[0xC9], true).expect("no error"), "");
        assert_eq!(
            decode_with(&host, &[], false).expect("no error"),
            "\u{FFFD}",
            "a non-streaming call flushes the held byte as a replacement"
        );
        assert_eq!(
            decode_with(&host, &[0xA9], false).expect("no error"),
            "\u{FFFD}",
            "and it replaced the decoder, so the continuation byte stands alone"
        );
    }

    /// A four-byte sequence split across three `{stream: true}` calls decodes
    /// to one character rather than to three replacements.
    #[test]
    fn text_decoder_decodes_a_split_four_byte_sequence_once() {
        let realm = host_realm();
        let host = decoder(&realm, None, false, false);

        assert_eq!(decode_with(&host, &[0xF0], true).expect("no error"), "");
        assert_eq!(
            decode_with(&host, &[0x9F, 0x98], true).expect("no error"),
            ""
        );
        assert_eq!(
            decode_with(&host, &[0x80], true).expect("no error"),
            "\u{1F600}",
            "the fourth byte completed the character"
        );
        assert_eq!(decode_with(&host, &[], false).expect("no error"), "");
    }

    /// An absent input is an empty input, and an absent options dictionary is
    /// its default — so `decode()`, `decode(undefined)`, `decode(undefined,
    /// undefined)` and `decode(undefined, {})` all reach the host as this one
    /// call, and all of them flush what a previous `{stream: true}` call held.
    #[test]
    fn text_decoder_flushes_on_an_absent_input() {
        let realm = host_realm();
        let host = decoder(&realm, None, false, false);

        assert_eq!(decode_with(&host, &[0xC9], true).expect("no error"), "");
        assert_eq!(
            <HostObject as TextDecoderOps>::decode(&host, None, None).expect("no error"),
            "\u{FFFD}",
            "an absent input still flushes the held byte"
        );
        assert_eq!(
            <HostObject as TextDecoderOps>::decode(
                &host,
                Some(Vec::new()),
                Some(Default::default())
            )
            .expect("no error"),
            "",
            "an explicit empty input on a flushed decoder is the empty string"
        );
    }

    /// `fatal` turns every malformed byte into a `TypeError` instead of a
    /// replacement character — including a sequence the flush completes, which
    /// is only malformed once it is known not to continue.
    #[test]
    fn text_decoder_fatal_rejects_malformed_input() {
        let realm = host_realm();
        let host = decoder(&realm, None, true, false);

        assert_eq!(
            <HostObject as TextDecoderOps>::decode(&host, Some(vec![0xFF]), None)
                .expect_err("0xFF opens no sequence in UTF-8")
                .kind(),
            JsErrorKind::Type
        );
        assert_eq!(
            decode_with(&host, &[0xC9], true).expect("not malformed yet"),
            "",
            "an incomplete sequence is not malformed while it may still continue"
        );
        assert_eq!(
            <HostObject as TextDecoderOps>::decode(&host, None, None)
                .expect_err("the flush completes a sequence that cannot")
                .kind(),
            JsErrorKind::Type
        );
        assert_eq!(
            decode(&host, &[0x41]),
            "A",
            "and a `fatal` decoder still decodes well-formed input"
        );
    }

    /// The decoder is the named encoding's, which is the point of taking
    /// `encoding_rs` rather than hand-writing UTF-8: 0x80 is `€` in
    /// windows-1252, and a UTF-16 pair is read in the named order.
    #[test]
    fn text_decoder_uses_the_named_encoding() {
        let realm = host_realm();
        assert_eq!(
            decode(
                &decoder(&realm, Some("windows-1252"), false, false),
                &[0x80]
            ),
            "\u{20AC}"
        );
        assert_eq!(
            decode(
                &decoder(&realm, Some("utf-16le"), false, false),
                &[0x41, 0x00]
            ),
            "A"
        );
        assert_eq!(
            decode(
                &decoder(&realm, Some("utf-16be"), false, false),
                &[0x00, 0x41]
            ),
            "A"
        );
    }

    /// The decoder and its state live in the record, so two `TextDecoder`s
    /// built from one label hold two of them.
    #[test]
    fn two_text_decoders_hold_their_own_streaming_state() {
        let realm = host_realm();
        let first = decoder(&realm, None, false, false);
        let second = decoder(&realm, None, false, false);

        assert_eq!(decode_with(&first, &[0xC9], true).expect("no error"), "");
        assert_eq!(
            decode_with(&second, &[], false).expect("no error"),
            "",
            "the second decoder never saw the byte the first is holding"
        );
        assert_eq!(
            decode_with(&first, &[0xA9], true).expect("no error"),
            "\u{269}",
            "and the first is still holding it"
        );
    }

    /// `TextEncoder` is the standard's UTF-8 encoder and reports the one
    /// encoding it can produce.
    #[test]
    fn text_encoder_encodes_utf8() {
        let realm = host_realm();
        let host = <HostObject as TextEncoderConstructor>::construct(&realm)
            .expect("the constructor takes no arguments");

        assert_eq!(
            <HostObject as TextEncoderOps>::encoding(&host).expect("an encoding has a name"),
            "utf-8"
        );
        assert_eq!(
            host.encode(Some("A\u{E9}\u{1F600}"))
                .expect("UTF-8 encodes"),
            vec![0x41, 0xC3, 0xA9, 0xF0, 0x9F, 0x98, 0x80],
            "one ASCII byte, one two-byte sequence and one four-byte sequence"
        );
        assert_eq!(
            host.encode(Some("")).expect("the empty string encodes"),
            Vec::<u8>::new()
        );
        assert_eq!(
            host.encode(None)
                .expect("an absent input is the empty string"),
            Vec::<u8>::new()
        );
    }

    // -----------------------------------------------------------------------
    // `Response`
    // -----------------------------------------------------------------------

    /// The settlement a `Body` member asked for, in the host's own terms.
    ///
    /// A [`PromiseOutcome`] borrows the labels it carries and there is no
    /// isolate here to build the value with, so the mock copies the outcome into
    /// this owned form and compares on it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Settled {
        Undefined,
        String(String),
        TypeError(String),
        Json(Vec<u8>),
    }

    /// A [`ScriptHost`] that records what the `Body` algorithms settle.
    ///
    /// `consume body` ends at `settle_promise`, so this is the last point every
    /// branch of `text()` and `json()` passes through, and recording it observes
    /// all of them without an isolate: the resolved text, the `TypeError` a
    /// second read rejects with, and the bytes `json()` asks the engine to parse.
    struct RecordingHost {
        /// How many promises have been asked for, which is also what makes the
        /// bits distinctive.
        made: std::cell::Cell<u64>,
        settled: RefCell<Vec<(u64, Settled)>>,
    }

    impl RecordingHost {
        fn new() -> Rc<Self> {
            Rc::new(Self {
                made: std::cell::Cell::new(0),
                settled: RefCell::new(Vec::new()),
            })
        }

        /// The one settlement that was requested, with the resolver whose
        /// promise it settles.
        fn only_settlement(&self) -> (u64, Settled) {
            let settled = self.settled.borrow();
            assert_eq!(settled.len(), 1, "one body read settles one promise");
            settled[0].clone()
        }
    }

    impl ScriptHost for RecordingHost {
        fn array_buffer_view_info(
            &self,
            _: crate::bindings::ArrayBufferViewRef,
        ) -> Result<crate::bindings::BufferMetadata, JsError> {
            unreachable!("this test never reads a native buffer")
        }
        fn write_array_buffer_view(
            &self,
            _: crate::bindings::ArrayBufferViewRef,
            _: &[u8],
        ) -> Result<crate::bindings::BufferMetadata, JsError> {
            unreachable!("this test never writes a native buffer")
        }
        fn check_native_buffer_work(&self) -> Result<(), JsError> {
            unreachable!("this test never performs native buffer work")
        }
        fn native_buffer_memory_failure(&self) -> JsError {
            unreachable!("this test never exhausts a native buffer budget")
        }

        fn invoke_listener(
            &self,
            _callback: CallbackRef,
            _current_target: u64,
            _event: u64,
        ) -> bool {
            unreachable!("no `Body` member dispatches an event");
        }

        fn release_value(&self, _bits: u64) {}

        fn retain_existing_target(&self, _handle: u64) -> Result<Option<u64>, JsError> {
            unreachable!("the Body settlement test has no EventTarget wrapper")
        }

        fn build_value(&self, _value: HostValue<'_>) -> Option<u64> {
            unreachable!("the settlement is recorded rather than built");
        }

        fn throw_value(&self, _bits: u64) {
            unreachable!("no `Body` member throws a stored value");
        }

        /// Distinctive bits per promise, so a test can tell which member made
        /// the promise a settlement names.
        fn new_promise(&self) -> Option<PromiseRef> {
            let made = self.made.get() + 1;
            self.made.set(made);
            Some(PromiseRef {
                promise: made * 2,
                resolver: made * 2 + 1,
            })
        }

        fn settle_promise(&self, resolver: u64, outcome: PromiseOutcome<'_>) -> bool {
            let settled = match outcome {
                PromiseOutcome::Resolve(HostValue::Undefined) => Settled::Undefined,
                PromiseOutcome::Resolve(HostValue::String(text)) => Settled::String(text),
                PromiseOutcome::Reject(HostValue::TypeError { message }) => {
                    Settled::TypeError(message.to_owned())
                }
                PromiseOutcome::ParseJson(bytes) => Settled::Json(bytes.to_vec()),
                other => panic!("no `Body` member settles with {other:?}"),
            };
            self.settled.borrow_mut().push((resolver, settled));
            true
        }

        /// Refused, because this host backs the `Body` algorithms and none of
        /// them is an `XMLHttpRequest`: there is no driver behind it for a
        /// request to reach, and the realm it serves has no page loop to drain
        /// a completion. It refuses rather than accepting so that a test which
        /// came to expect a start observes the failure the contract names
        /// (["`Err` means no request was started for any
        /// reason"](crate::dom::events::ScriptHost::start_xhr)) instead of
        /// passing vacuously on a request nothing ever answered.
        fn start_xhr(
            &self,
            _request: crate::xhr::XhrRequest,
            _handle: crate::xhr::XhrHandle,
        ) -> Result<(), crate::fetch::FetchStartError> {
            Err(crate::fetch::FetchStartError::new(
                "the recording host starts no XMLHttpRequest",
            ))
        }

        /// `false` for the same reason [`Self::start_xhr`] refuses: a host that
        /// never started a request never holds one to end.
        fn abort_xhr(&self, _handle: crate::xhr::XhrHandle) -> bool {
            false
        }

        /// Unreachable: nothing this host serves arms a timer, so nothing it
        /// does can schedule a background task. A task that arrived here would
        /// have to run, so it panics rather than being dropped silently — the
        /// same shape as this host's other unreachable members.
        fn schedule_background_task(&self, _delay_ms: u64, _task: Box<dyn FnOnce()>) {
            unreachable!("no `Body` member arms a timer");
        }
    }

    /// A [`Realm`] with the recording script host installed, which is what lets
    /// the `Body` algorithms run past their promise creation.
    fn body_realm() -> (Realm, Rc<RecordingHost>) {
        let realm = host_realm();
        let host = RecordingHost::new();
        *realm_context(&realm)
            .expect("the realm is a host context")
            .script
            .borrow_mut() = Some(host.clone());
        (realm, host)
    }

    /// `new Response(body, init)`.
    fn response(
        realm: &Realm,
        body: Option<BodyInit>,
        init: ResponseInit,
    ) -> Result<HostObject, JsError> {
        <HostObject as ResponseConstructor>::construct(realm, Some(body), Some(init))
    }

    /// The header list the response's own `Headers` holds, read through the
    /// `[SameObject]` getter rather than off the record, so the test observes
    /// what script would.
    fn response_headers(realm: &Realm, response: &HostObject) -> Vec<(Vec<u8>, Vec<u8>)> {
        let bits = <HostObject as ResponseOps>::headers(response)
            .expect("a constructed response has a headers object")
            .bits();
        let decoded = super::host::decode(bits);
        assert_eq!(
            decoded.tag,
            super::host::TAG_HOST,
            "the handle is a JS-owned host object"
        );
        let headers = HostObject::host_record(
            realm_context(realm).expect("the realm is a host context"),
            super::host::HostKey {
                index: decoded.node_id.0,
                generation: decoded.generation,
            },
        );
        <HostObject as HeadersOps>::iterable_entries(&headers).expect("a headers list")
    }

    /// A pair list, as `Headers`' own constructor builds one.
    fn pairs(pairs: &[(&str, &str)]) -> HeadersInit {
        HeadersInit::SequenceOfSequenceOfByteString(
            pairs
                .iter()
                .map(|(name, value)| vec![name.as_bytes().to_vec(), value.as_bytes().to_vec()])
                .collect(),
        )
    }

    /// `initialize a response` steps 1 and 2, which run before anything is
    /// inserted and which are the only two members the constructor validates
    /// rather than converts.
    #[test]
    fn response_validates_the_status_and_the_status_text() {
        let realm = host_realm();
        let status = |status: u16| {
            response(
                &realm,
                None,
                ResponseInit {
                    status,
                    ..Default::default()
                },
            )
            .err()
            .map(|error| error.kind())
        };

        // The range is 200 to 599 inclusive: 199 and 600 are the two ends that
        // fail, and the two boundaries themselves are the two that pass.
        assert_eq!(status(199), Some(JsErrorKind::Range));
        assert_eq!(status(600), Some(JsErrorKind::Range));
        assert_eq!(status(200), None);
        assert_eq!(status(599), None);
        // The default is 200, so a dictionary that names nothing is valid.
        assert_eq!(response(&realm, None, Default::default()).err(), None);

        // The status text is a `ByteString`, so 0x80 arrives as a byte rather
        // than as a character, and *reason-phrase* is the reason a control
        // character or a non-ASCII byte is refused.
        let text = |text: &[u8]| {
            response(
                &realm,
                None,
                ResponseInit {
                    status_text: text.to_vec(),
                    ..Default::default()
                },
            )
            .err()
            .map(|error| error.kind())
        };
        assert_eq!(text(b""), None, "the empty string is always allowed");
        assert_eq!(text(b"OK"), None);
        assert_eq!(text(b"not\tfound"), None, "HTAB is part of the production");
        assert_eq!(text(b"a b"), None, "so is SP");
        assert_eq!(text(&[0xFF]), None, "and so is obs-text");
        assert_eq!(text(b"line\nbreak"), Some(JsErrorKind::Type));
        assert_eq!(text(b"null\0byte"), Some(JsErrorKind::Type));
        assert_eq!(text(&[0x7F]), Some(JsErrorKind::Type));
    }

    /// The constructor's steps 1 and 2 make the response and its headers object
    /// before `initialize a response` can throw, so a straightforward reading
    /// inserts two records for a construction that then fails. Both checks here
    /// run before the first insert, which is what makes a rejected construction
    /// leave the slab as it found it.
    #[test]
    fn a_rejected_construction_leaves_no_record_behind() {
        let realm = host_realm();
        let ctx = realm_context(&realm).expect("the realm is a host context");
        let status = |status: u16| {
            response(
                &realm,
                None,
                ResponseInit {
                    status,
                    ..Default::default()
                },
            )
            .err()
            .expect("the status is outside the range")
            .kind()
        };
        let text = |text: &[u8]| {
            response(
                &realm,
                None,
                ResponseInit {
                    status_text: text.to_vec(),
                    ..Default::default()
                },
            )
            .err()
            .expect("the status text is not a reason phrase")
            .kind()
        };

        assert_eq!(status(199), JsErrorKind::Range);
        assert_eq!(status(600), JsErrorKind::Range);
        assert_eq!(text(b"line\nbreak"), JsErrorKind::Type);
        assert_eq!(
            ctx.host_records.borrow().len(),
            0,
            "neither the response nor its headers object was inserted"
        );

        // And a construction that gets past both is the two records it takes:
        // the `Headers` the response owns, then the response.
        response(&realm, None, Default::default()).expect("a valid response");
        assert_eq!(ctx.host_records.borrow().len(), 2);
    }

    /// Step 6's body rules: a null body status cannot carry a body, a string
    /// body contributes a `Content-Type`, and a buffer source contributes none.
    #[test]
    fn response_applies_the_body_rules_of_initialize_a_response() {
        let realm = host_realm();
        let of = |body: BodyInit, init: ResponseInit| response(&realm, Some(body), init);

        // The five null body statuses. Two of them — 101 and 103 — are below
        // the status range, so step 1 of *initialize a response* rejects them
        // before step 6 ever asks about a body; the other three reach the rule
        // this test is about.
        for status in [101u16, 103] {
            assert_eq!(
                of(
                    BodyInit::UsvString("hi".to_owned()),
                    ResponseInit {
                        status,
                        ..Default::default()
                    },
                )
                .err()
                .expect("the status is outside the range")
                .kind(),
                JsErrorKind::Range,
                "status {status} fails the range check first"
            );
        }
        for status in [204u16, 205, 304] {
            let error = of(
                BodyInit::UsvString("hi".to_owned()),
                ResponseInit {
                    status,
                    ..Default::default()
                },
            )
            .err()
            .expect("a null body status cannot have a body");
            assert_eq!(error.kind(), JsErrorKind::Type, "status {status}");
            // A status the range check still lets through, with no body at all:
            // the `Some(None)` an explicit `null` argument is.
            assert_eq!(
                <HostObject as ResponseConstructor>::construct(
                    &realm,
                    Some(None),
                    Some(ResponseInit {
                        status,
                        ..Default::default()
                    }),
                )
                .err(),
                None,
                "status {status}: a null body — not an empty one — is what is allowed"
            );
        }

        let text = of(BodyInit::UsvString("hi".to_owned()), Default::default())
            .expect("the body is valid");
        assert_eq!(
            response_headers(&realm, &text),
            vec![(
                b"content-type".to_vec(),
                b"text/plain;charset=UTF-8".to_vec()
            )],
            "a `USVString` body's type is appended to the header list"
        );

        let bytes = of(BodyInit::BufferSource(vec![b'h', b'i']), Default::default())
            .expect("the body is valid");
        assert_eq!(
            response_headers(&realm, &bytes),
            Vec::new(),
            "a buffer source's body has no type, so nothing is appended"
        );

        // The append is skipped when the list already names `Content-Type`,
        // whatever case the caller spelled it in.
        let named = of(
            BodyInit::UsvString("hi".to_owned()),
            ResponseInit {
                headers: Some(pairs(&[("Content-Type", "text/plain")])),
                ..Default::default()
            },
        )
        .expect("the body is valid");
        assert_eq!(
            response_headers(&realm, &named),
            vec![(b"content-type".to_vec(), b"text/plain".to_vec())]
        );
    }

    /// The `"response"` guard's own rule, which is the one thing the `Response`
    /// constructor's *fill* does that `Headers`' constructor's does not.
    #[test]
    fn the_response_guard_drops_the_forbidden_response_header_names() {
        let realm = host_realm();
        // `Set-Cookie` and `Set-Cookie2` are the response guard's forbidden
        // names, and the drop is a *silent* return: Fetch's guarded append does
        // not throw.
        let with_response_guard = response(
            &realm,
            None,
            ResponseInit {
                headers: Some(pairs(&[
                    ("Set-Cookie", "a=b"),
                    ("set-cookie2", "c=d"),
                    ("Set-Cookie3", "kept"),
                    ("X-Other", "kept"),
                ])),
                ..Default::default()
            },
        )
        .expect("a forbidden name is dropped, not thrown on");
        assert_eq!(
            response_headers(&realm, &with_response_guard),
            vec![
                (b"set-cookie3".to_vec(), b"kept".to_vec()),
                (b"x-other".to_vec(), b"kept".to_vec()),
            ],
            "the two forbidden names are gone and nothing else is"
        );

        // The same names under the `Headers` constructor's guard are kept, so
        // the assertions above are about the guard and not about the names.
        let immutable = <HostObject as HeadersConstructor>::construct(
            &realm,
            Some(pairs(&[("Set-Cookie", "a=b"), ("set-cookie2", "c=d")])),
        )
        .expect("the `none` guard permits both names");
        assert_eq!(
            <HostObject as HeadersOps>::iterable_entries(&immutable).expect("a headers list"),
            vec![
                (b"set-cookie".to_vec(), b"a=b".to_vec()),
                (b"set-cookie2".to_vec(), b"c=d".to_vec()),
            ]
        );

        // The guard is the record's, so it outlives the constructor: the same
        // *append* is dropped on the response's headers object and kept on the
        // one the page built.
        let built = response(&realm, None, Default::default()).expect("a usable response");
        let bits = <HostObject as ResponseOps>::headers(&built)
            .expect("a headers handle")
            .bits();
        let decoded = super::host::decode(bits);
        let headers = HostObject::host_record(
            realm_context(&realm).expect("the realm is a host context"),
            super::host::HostKey {
                index: decoded.node_id.raw(),
                generation: decoded.generation,
            },
        );
        <HostObject as HeadersOps>::append(&headers, b"Set-Cookie", b"a=b").expect("no throw");
        <HostObject as HeadersOps>::append(&headers, b"X-Other", b"1").expect("no throw");
        <HostObject as HeadersOps>::append(&immutable, b"Set-Cookie", b"z=z").expect("no throw");
        assert_eq!(
            <HostObject as HeadersOps>::iterable_entries(&headers).expect("a headers list"),
            vec![(b"x-other".to_vec(), b"1".to_vec())],
            "the guarded append dropped the forbidden name and kept the other"
        );
        assert_eq!(
            <HostObject as HeadersOps>::iterable_entries(&immutable).expect("a headers list"),
            vec![
                (b"set-cookie".to_vec(), b"a=b".to_vec()),
                // `set-cookie` is the one name the iterator does not combine,
                // so its two values stay two pairs.
                (b"set-cookie".to_vec(), b"z=z".to_vec()),
                (b"set-cookie2".to_vec(), b"c=d".to_vec()),
            ],
            "and the unguarded one appended a second pair rather than being dropped"
        );
    }

    /// The three `ResponseType` values a constructor-built response reports are
    /// the standard's defaults, and `ok` is HTTP's own success range.
    #[test]
    fn a_constructed_response_reports_the_documented_defaults() {
        let realm = host_realm();
        let built = response(&realm, None, Default::default()).expect("the empty response");

        assert_eq!(
            <HostObject as ResponseOps>::r#type(&built).expect("an enumeration value"),
            ResponseType::Default
        );
        assert_eq!(
            <HostObject as ResponseOps>::url(&built).expect("a URL string"),
            "",
            "a constructor-built response has no URL, so `url` is the empty string"
        );
        assert!(!<HostObject as ResponseOps>::redirected(&built));
        assert_eq!(<HostObject as ResponseOps>::status(&built), 200);
        assert!(<HostObject as ResponseOps>::ok(&built));
        assert_eq!(
            <HostObject as ResponseOps>::status_text(&built).expect("a status text"),
            Vec::<u8>::new()
        );
        assert!(!<HostObject as ResponseOps>::body_used(&built));

        // `ok` is *ok status*: 200 to 299 inclusive, so the two boundaries are
        // the ones worth reading and 300 is the first status outside it.
        let with_status = |status: u16| {
            response(
                &realm,
                None,
                ResponseInit {
                    status,
                    ..Default::default()
                },
            )
            .expect("a status inside the range")
        };
        assert!(
            <HostObject as ResponseOps>::ok(&with_status(299)),
            "299 is the last ok status"
        );
        assert!(
            !<HostObject as ResponseOps>::ok(&with_status(300)),
            "300 is the first status that is not ok"
        );
        assert_eq!(
            <HostObject as ResponseOps>::status(&with_status(599)),
            599,
            "and the status is reported as it was given"
        );
    }

    /// `[SameObject] headers`: the getter answers with the key the constructor
    /// stored, so two reads are the same record and therefore the same wrapper.
    #[test]
    fn the_headers_getter_is_the_same_object_every_time() {
        let realm = host_realm();
        let built = response(&realm, None, Default::default()).expect("the empty response");
        let first = <HostObject as ResponseOps>::headers(&built).expect("a headers handle");
        let second = <HostObject as ResponseOps>::headers(&built).expect("a headers handle");
        assert_eq!(first, second, "the two reads are the same wrapper bits");
    }

    /// `consume body`'s step 1: an object whose body has already been read is
    /// *unusable*, and the rejection is the same whether the member is `text()`
    /// or `json()`.
    #[test]
    fn a_second_body_read_is_rejected_as_unusable() {
        let (realm, script) = body_realm();
        let built = response(
            &realm,
            Some(BodyInit::UsvString("hi".to_owned())),
            Default::default(),
        )
        .expect("a string body");

        // The first read resolves with the text and disturbs the body.
        let promise = <HostObject as ResponseOps>::text(&built).expect("a promise");
        assert_eq!(promise.bits(), 2, "the first promise the host made");
        assert_eq!(
            script.only_settlement(),
            (3, Settled::String("hi".to_owned()))
        );
        assert!(
            <HostObject as ResponseOps>::body_used(&built),
            "the disturbance is observable synchronously, before the settlement runs"
        );

        // The second read takes step 1: it makes its own promise and rejects
        // that one, before any conversion runs. Both halves are asserted,
        // because a member that threw instead of rejecting would not have made
        // a promise at all, and one that converted anyway would not have the
        // rejection.
        let again = <HostObject as ResponseOps>::text(&built).expect("a rejected promise");
        assert_eq!(
            script.made.get(),
            2,
            "the second read made a second promise"
        );
        assert_eq!(again.bits(), 4, "and returned it");
        assert_eq!(
            script.settled.borrow()[1],
            (
                5,
                Settled::TypeError("The body has already been read".to_owned())
            )
        );
        assert!(
            <HostObject as ResponseOps>::body_used(&built),
            "a failed read leaves the body disturbed"
        );

        // `json()` is the same algorithm and takes the same step.
        <HostObject as ResponseOps>::json(&built).expect("a rejected promise");
        assert_eq!(
            script.settled.borrow()[2],
            (
                7,
                Settled::TypeError("The body has already been read".to_owned())
            )
        );
    }

    /// A null body is the empty byte sequence, so `text()` resolves `""` and
    /// `json()` rejects — the empty string is not JSON.
    #[test]
    fn a_null_body_reads_as_the_empty_byte_sequence() {
        let (realm, script) = body_realm();
        let built = response(&realm, None, Default::default()).expect("the empty response");

        <HostObject as ResponseOps>::text(&built).expect("a promise");
        assert_eq!(
            script.only_settlement(),
            (3, Settled::String(String::new()))
        );
        assert!(
            !<HostObject as ResponseOps>::body_used(&built),
            "a null body is never disturbed: `bodyUsed` is false for it however often it is read"
        );

        <HostObject as ResponseOps>::json(&built).expect("a promise that will reject");
        assert_eq!(
            script.settled.borrow()[1],
            (5, Settled::Json(Vec::new())),
            "`json()` asks the engine to parse the empty sequence, and the parse is what rejects"
        );
    }

    /// `json()` converts bytes rather than a value, so the conversion is the
    /// engine's and the host hands it the *body*, not a decoded string.
    #[test]
    fn json_hands_the_engine_the_body_bytes() {
        let (realm, script) = body_realm();
        let built = response(
            &realm,
            Some(BodyInit::UsvString("{\"a\":1}".to_owned())),
            Default::default(),
        )
        .expect("a JSON body");

        <HostObject as ResponseOps>::json(&built).expect("a promise");
        let (resolver, settled) = script.only_settlement();
        assert_eq!(resolver, 3);
        assert_eq!(settled, Settled::Json(b"{\"a\":1}".to_vec()));
    }

    /// A delivered completion clears the send flag, whichever way it finished.
    ///
    /// This is the half of the flag's lifetime a page cannot observe on its own:
    /// `send()`'s step 1 refuses anything that is not at `OPENED`, and *every*
    /// completion — a response and a failure alike — leaves the object at `DONE`,
    /// so the state check answers before the flag check can and script reads
    /// `InvalidStateError` either way. The record is therefore the only place the
    /// clear is visible, and this test is that place.
    ///
    /// Each of the completion's two branches clears it inside its own algorithm
    /// now — *handle response end-of-body* step 8 in `xhr_handle_response`, and
    /// the *request error steps* step 2 in `xhr_request_error_steps`, which is
    /// where a fired `timeout` and an `abort()` clear it too. This test reaches
    /// the `Failed` branch, so the algorithm it pins is the error one.
    #[test]
    fn a_delivered_completion_clears_the_send_flag() {
        use super::host::{HostRecord, deliver_xhr_completion};
        use crate::dom::events::DispatchTarget;
        use crate::fetch::FetchOutcome;

        let realm = host_realm();
        let ctx = realm_context(&realm).expect("the realm is a host context");
        let host = <HostObject as XMLHttpRequestConstructor>::construct(&realm)
            .expect("a new XMLHttpRequest is constructible");
        let HostObject::Host { key, .. } = host else {
            panic!("an XMLHttpRequest is a host-record wrapper");
        };
        {
            let mut records = ctx.host_records.borrow_mut();
            let Some(HostRecord::Xhr(record)) = records.get_mut(key) else {
                panic!("the constructor installs an XMLHttpRequest record");
            };
            record.ready_state = <HostObject as XMLHttpRequestOps>::OPENED;
            record.send_flag = true;
            record.request_id = 3;
        }

        let delivered = deliver_xhr_completion(
            &ctx,
            DispatchTarget::host(ctx.slot, key),
            3,
            &FetchOutcome::Failed("gone".to_owned()),
        );
        assert!(delivered, "the record names an XMLHttpRequest");

        let records = ctx.host_records.borrow();
        let Some(HostRecord::Xhr(record)) = records.get(key) else {
            panic!("the delivered completion must not remove the record");
        };
        assert!(
            !record.send_flag,
            "a finished request is no longer in flight, so a re-opened object can be sent"
        );
    }

    /// A completion belonging to a request its object has abandoned is
    /// discarded: nothing is stored, no state changes, and the return is `true`
    /// so the caller fires no fallback event at it either.
    ///
    /// This is `open()`'s step 10 and `abort()`'s step 1 as this product serves
    /// them together: both end the request through `end_xhr_request`, which
    /// terminates it at the driver *and* moves the record's id on. The id is the
    /// half that has to be read here, because only it refuses an answer that was
    /// already in the driver's table when the termination landed — the driver is
    /// on the other side of the seam (`crate::xhr::XhrDriver`), and an answer can
    /// be pushed a moment before the abort reaches it. This is the comparison
    /// that reads it (`HostRecord::Xhr`'s `request_id`).
    #[test]
    fn a_completion_for_an_abandoned_request_is_discarded() {
        use super::host::{HostRecord, deliver_xhr_completion};
        use crate::dom::events::DispatchTarget;
        use crate::fetch::FetchOutcome;

        let realm = host_realm();
        let ctx = realm_context(&realm).expect("the realm is a host context");
        let host = <HostObject as XMLHttpRequestConstructor>::construct(&realm)
            .expect("a new XMLHttpRequest is constructible");
        let HostObject::Host { key, .. } = host else {
            panic!("an XMLHttpRequest is a host-record wrapper");
        };
        {
            let mut records = ctx.host_records.borrow_mut();
            let Some(HostRecord::Xhr(record)) = records.get_mut(key) else {
                panic!("the constructor installs an XMLHttpRequest record");
            };
            // A request is in flight (`send()` stamped 2), and `open()` has run
            // since (bumping to 3), which is what abandons it.
            record.ready_state = <HostObject as XMLHttpRequestOps>::OPENED;
            record.send_flag = true;
            record.request_id = 3;
        }
        let events_before = ctx.events.borrow().len();

        let delivered = deliver_xhr_completion(
            &ctx,
            DispatchTarget::host(ctx.slot, key),
            2,
            &FetchOutcome::Response(crate::fetch::FetchedResponse {
                url: "http://origin.test/x".to_owned(),
                status: 200,
                redirected: false,
                headers: Vec::new(),
                body: Some(b"the abandoned request's body".to_vec()),
            }),
        );

        assert!(
            delivered,
            "the target *is* an XMLHttpRequest, so the caller's fallback event is \
             suppressed as well"
        );
        let records = ctx.host_records.borrow();
        let Some(HostRecord::Xhr(record)) = records.get(key) else {
            panic!("a discarded completion must not remove the record");
        };
        assert_eq!(
            record.ready_state,
            <HostObject as XMLHttpRequestOps>::OPENED,
            "the abandoned response must not move the live request's state"
        );
        assert!(
            record.response.is_none(),
            "and must not become the object's response"
        );
        assert!(
            record.send_flag,
            "the live request is still in flight, so its flag stays set"
        );
        drop(records);
        assert_eq!(
            ctx.events.borrow().len(),
            events_before,
            "and no event is fired for it, not even a bare load or error"
        );
    }
}

/// The `fetch()` initializer's rules: the Fetch Standard's *Request* constructor
/// steps this engine runs (`super::host::request_from_init`), and the refusal of
/// the members it does not serve.
///
/// They are engine-independent on purpose, and that is what makes this the place
/// for them: which of the dictionary's fifteen members is served, what a method
/// normalises to, whether a body is allowed, and what a body contributes are all
/// decided over the *converted* dictionary, so none of them needs an isolate.
/// The half that does — the argument conversion, i.e. what V8 value becomes a
/// `RequestInit` at all, and the `TypeError` a value that is not an object gets —
/// is asserted against a real engine in
/// `crates/tenta-js-v8/tests/dom_fetch.rs`.
#[cfg(test)]
mod request_init {
    use super::host::{HostContext, refused_request_init_member, request_from_init};
    use super::*;
    use crate::event_loop::{Clock, MonoTime, SharedClock};
    use crate::fetch::FetchRequest;
    use std::cell::RefCell;
    use std::rc::Rc;
    use tenta_core::Origin;
    use tenta_dom::Document;
    use url::Url;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("a test URL")
    }

    /// The request `init` builds for a document at `http://origin.test/x`.
    fn request_for(init: RequestInit) -> Result<FetchRequest, JsError> {
        let url = url("http://origin.test/x");
        // A body arm that names a record is looked up in a context; none of
        // these requests has one, so the context is an empty one.
        let ctx = HostContext::new(
            Rc::new(RefCell::new(Document::new())),
            SharedClock::new(Clock::virtual_at(MonoTime::ZERO)),
            tenta_net::CookieJar::new(),
            crate::storage::PageStorage::isolated(),
        );
        request_from_init(&ctx, init, url.clone(), url, Origin::Tracked)
    }

    /// An initializer naming one `method` and nothing else.
    fn method_of(bytes: &[u8]) -> RequestInit {
        RequestInit {
            method: Some(bytes.to_vec()),
            ..Default::default()
        }
    }

    /// An initializer with a `POST` method and `body` — the shape a request with
    /// a payload needs, because a `GET` may not carry one.
    fn post(body: Option<BodyInit>) -> RequestInit {
        RequestInit {
            method: Some(b"POST".to_vec()),
            body: Some(body),
            ..Default::default()
        }
    }

    /// A body's bytes as the `BufferSource` arm of `BodyInit`.
    fn bytes(body: &[u8]) -> BodyInit {
        BodyInit::BufferSource(body.to_vec())
    }

    fn text(body: &str) -> BodyInit {
        BodyInit::UsvString(body.to_owned())
    }

    /// The `headers` member from a page's own pairs.
    fn header_init(pairs: &[(&str, &str)]) -> HeadersInit {
        HeadersInit::SequenceOfSequenceOfByteString(
            pairs
                .iter()
                .map(|(name, value)| vec![name.as_bytes().to_vec(), value.as_bytes().to_vec()])
                .collect(),
        )
    }

    /// The header list as the page-visible pairs the driver would send: names
    /// are byte-lowercased by the time anything is stored.
    fn headers(request: &FetchRequest) -> Vec<(&str, &str)> {
        request
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect()
    }

    /// The empty initializer is the whole of `fetch(url)`: a `GET` with no
    /// headers and no body. `fetch(url, {})`, `fetch(url, null)` and
    /// `fetch(url, undefined)` are the same value, so this is also their test.
    #[test]
    fn an_empty_initializer_is_a_get_with_nothing_on_it() {
        let request = request_for(RequestInit::default()).expect("an empty initializer is valid");

        assert_eq!(request.method, http::Method::GET);
        assert!(request.headers.is_empty());
        assert!(request.body.is_none());
        assert_eq!(
            refused_request_init_member(&RequestInit::default()),
            None,
            "an empty dictionary names no member, so nothing is refused"
        );
    }

    /// A member is refused, *by name*, when its value asks for a request this
    /// engine would not send as written.
    #[test]
    fn every_value_this_engine_cannot_serve_is_refused_by_name() {
        let cases: [(&str, RequestInit); 7] = [
            (
                "referrer",
                RequestInit {
                    referrer: Some("https://elsewhere.test/".to_owned()),
                    ..Default::default()
                },
            ),
            (
                "referrerPolicy",
                RequestInit {
                    referrer_policy: Some(ReferrerPolicy::NoReferrer),
                    ..Default::default()
                },
            ),
            (
                "mode",
                RequestInit {
                    mode: Some(RequestMode::Navigate),
                    ..Default::default()
                },
            ),
            (
                "cache",
                RequestInit {
                    cache: Some(RequestCache::OnlyIfCached),
                    ..Default::default()
                },
            ),
            (
                "redirect",
                RequestInit {
                    redirect: Some(RequestRedirect::Manual),
                    ..Default::default()
                },
            ),
            (
                "integrity",
                RequestInit {
                    integrity: Some("sha256-abc".to_owned()),
                    ..Default::default()
                },
            ),
            // `null` is the only value the standard allows `window`.
            (
                "window",
                RequestInit {
                    window: Some(AnyValue(AnyValue::NULL.0 ^ 1)),
                    ..Default::default()
                },
            ),
        ];

        for (name, init) in cases {
            assert_eq!(
                refused_request_init_member(&init),
                Some(name),
                "the member `{name}` is not served with this value, so it must be refused by name"
            );
        }
    }

    /// The members the engine serves, and the defaults written out, name the
    /// request it sends anyway and are accepted; the modes reach the request.
    #[test]
    fn served_members_and_written_out_defaults_are_accepted() {
        let init = RequestInit {
            referrer: Some("about:client".to_owned()),
            referrer_policy: Some(ReferrerPolicy::EmptyString),
            mode: Some(RequestMode::SameOrigin),
            credentials: Some(RequestCredentials::Include),
            cache: Some(RequestCache::NoStore),
            redirect: Some(RequestRedirect::Error),
            integrity: Some(String::new()),
            keepalive: Some(true),
            signal: Some(None),
            duplex: Some(RequestDuplex::Half),
            priority: Some(RequestPriority::Low),
            window: Some(AnyValue::NULL),
            ..Default::default()
        };
        assert_eq!(refused_request_init_member(&init), None);

        let request = request_for(init).expect("a served initializer");
        assert_eq!(request.credentials, crate::fetch::Credentials::Include);
        assert_eq!(request.mode, crate::fetch::Mode::SameOrigin);
        assert_eq!(request.redirect, crate::fetch::Redirect::Error);
        assert!(request.bypass_cache);

        let plain = request_for(RequestInit::default()).expect("an empty initializer");
        assert_eq!(plain.credentials, crate::fetch::Credentials::SameOrigin);
        assert_eq!(plain.mode, crate::fetch::Mode::Cors);
        assert_eq!(plain.redirect, crate::fetch::Redirect::Follow);
        assert!(!plain.bypass_cache);
    }

    /// Only one name is reported however many members are named, and it is the
    /// first in the standard's own declaration order — so the refusal is a
    /// function of the dictionary and not of insertion order in the call.
    #[test]
    fn the_first_refused_member_in_declaration_order_is_the_one_named() {
        let init = RequestInit {
            window: Some(AnyValue::NULL),
            keepalive: Some(true),
            mode: Some(RequestMode::Navigate),
            // `method` and `body` are served, so naming them changes nothing.
            method: Some(b"POST".to_vec()),
            body: Some(Some(text("hi"))),
            ..Default::default()
        };

        assert_eq!(refused_request_init_member(&init), Some("mode"));
    }

    /// `normalize a method` byte-uppercases the six it names and leaves every
    /// other method in the case the page wrote it in
    /// (<https://fetch.spec.whatwg.org/#concept-method-normalize>).
    #[test]
    fn a_method_is_normalized_only_where_the_standard_says_so() {
        let normalized = |bytes: &[u8]| {
            request_for(method_of(bytes))
                .expect("a valid method")
                .method
                .as_str()
                .to_owned()
        };

        // The six the standard uppercases, spelled the way a page would.
        assert_eq!(normalized(b"get"), "GET");
        assert_eq!(normalized(b"pOsT"), "POST");
        assert_eq!(normalized(b"delete"), "DELETE");
        assert_eq!(normalized(b"options"), "OPTIONS");
        assert_eq!(normalized(b"head"), "HEAD");
        assert_eq!(normalized(b"put"), "PUT");
        // And everything else keeps its case. `patch` is the standard's own
        // example: "PATCH is much more likely to succeed".
        assert_eq!(normalized(b"patch"), "patch");
        assert_eq!(normalized(b"CHICKEN"), "CHICKEN");
        assert_eq!(normalized(b"chicken"), "chicken");
    }

    /// A value that is not a *method* is a `TypeError`, and the *method*
    /// production is HTTP's `token`: non-empty and made of `tchar`s only
    /// (RFC 9110 §5.6.2).
    #[test]
    fn a_value_that_is_not_a_method_token_is_a_type_error() {
        for bytes in [b"".as_slice(), b"GE T", b"GET\t", b"g\xc3\xa9t", b"GET\x00"] {
            let error = request_for(method_of(bytes)).expect_err("not a method token");
            assert_eq!(error.kind(), JsErrorKind::Type, "method {bytes:?}");
            assert!(
                error.message().contains("not a valid HTTP method token"),
                "unexpected message for {bytes:?}: {}",
                error.message()
            );
        }
    }

    /// `CONNECT`, `TRACE` and `TRACK` are the *forbidden methods*, matched
    /// byte-case-insensitively, and the exception is a `TypeError` — not XHR's
    /// `SecurityError`, which is a different standard's answer to the same
    /// question (<https://fetch.spec.whatwg.org/#forbidden-method>).
    #[test]
    fn a_forbidden_method_is_a_type_error() {
        for bytes in [b"CONNECT".as_slice(), b"trace", b"Track"] {
            let error = request_for(method_of(bytes)).expect_err("a forbidden method");
            assert_eq!(error.kind(), JsErrorKind::Type, "method {bytes:?}");
            assert!(
                error.message().contains("forbidden"),
                "unexpected message for {bytes:?}: {}",
                error.message()
            );
        }
    }

    /// "If either `init["body"]` exists and is non-null … and request's method is
    /// `GET` or `HEAD`, then throw a `TypeError`"
    /// (<https://fetch.spec.whatwg.org/#dom-request>, the constructor's body
    /// step). A null body is not a body, and `POST` is not `GET`.
    #[test]
    fn a_body_on_a_get_or_a_head_is_a_type_error() {
        let with = |method: &[u8], body: Option<BodyInit>| {
            request_for(RequestInit {
                method: Some(method.to_vec()),
                body: Some(body),
                ..Default::default()
            })
        };

        for method in [b"GET".as_slice(), b"get", b"HEAD"] {
            let error = with(method, Some(text("hi"))).expect_err("a body on a GET or HEAD");
            assert_eq!(error.kind(), JsErrorKind::Type, "method {method:?}");
            assert!(
                error.message().contains("GET or a HEAD"),
                "unexpected message: {}",
                error.message()
            );
            // A null body is explicitly not a body, so the same method is fine.
            assert!(
                with(method, None).is_ok(),
                "a null body is not a body for {method:?}"
            );
        }

        assert!(
            with(b"POST", Some(text("hi"))).is_ok(),
            "a POST is the request a body is for"
        );
        assert!(
            with(b"PATCH", Some(bytes(b"x"))).is_ok(),
            "and so is any other method the token production admits"
        );
    }

    /// *Extract a body with type* for the two arms `BodyInit` has: a scalar
    /// value string becomes its UTF-8 bytes **and** a `Content-Type`, and a
    /// buffer source becomes its bytes and nothing else
    /// (<https://fetch.spec.whatwg.org/#concept-bodyinit-extract>).
    #[test]
    fn a_string_body_contributes_a_content_type_and_a_buffer_source_does_not() {
        let request = request_for(post(Some(text("héllo")))).expect("a POST with a body");
        assert_eq!(request.body, Some("héllo".as_bytes().to_vec()));
        assert_eq!(
            headers(&request),
            vec![("content-type", "text/plain;charset=UTF-8")],
            "the string arm infers the standard's type and it is appended"
        );

        let request =
            request_for(post(Some(bytes(&[0xff, 0x00, 0x41])))).expect("a POST with a body");
        assert_eq!(request.body, Some(vec![0xff, 0x00, 0x41]));
        assert!(
            request.headers.is_empty(),
            "a buffer source contributes no type, so nothing is appended"
        );
    }

    /// The append is skipped when the list already names `Content-Type`,
    /// whatever case the page spelled it in — names are stored lowercased, so
    /// the test for it is a plain equality.
    #[test]
    fn a_content_type_the_page_named_is_not_replaced() {
        let request = request_for(RequestInit {
            headers: Some(header_init(&[("Content-Type", "text/plain")])),
            ..post(Some(text("hi")))
        })
        .expect("a POST with a body");

        assert_eq!(
            headers(&request),
            vec![("content-type", "text/plain")],
            "the page's own `Content-Type` is the only one"
        );
    }

    /// The initializer's headers are *filled* through the `"request"` guard, so
    /// a *forbidden request-header* is dropped silently rather than thrown on.
    /// This is the half that keeps the transport the only author of the framing
    /// header: a page's `Content-Length` never reaches a driver.
    #[test]
    fn the_initializer_headers_are_filled_under_the_request_guard() {
        let request = request_for(RequestInit {
            headers: Some(header_init(&[
                ("X-Token", "abc"),
                ("Content-Length", "999"),
                ("Host", "elsewhere.test"),
                ("Cookie", "a=b"),
                ("Proxy-Authorization", "secret"),
                ("X-Forwarded-For", "1.2.3.4"),
            ])),
            ..Default::default()
        })
        .expect("filling a request list never throws");

        assert_eq!(
            headers(&request),
            vec![("x-token", "abc"), ("x-forwarded-for", "1.2.3.4")],
            "the four forbidden names are gone and the other two are not"
        );
    }

    /// The value-dependent half of the forbidden request-header rule survives
    /// the guard: `X-HTTP-Method-Override` is forbidden only when its value
    /// names a forbidden method, so the pair test is not the name test.
    #[test]
    fn a_method_override_header_is_forbidden_only_for_a_forbidden_method() {
        let filled = |value: &str| {
            request_for(RequestInit {
                headers: Some(header_init(&[("X-HTTP-Method-Override", value)])),
                ..Default::default()
            })
            .expect("filling a request list never throws")
        };

        assert!(
            headers(&filled("TRACE")).is_empty(),
            "`TRACE` is a forbidden method, so the header is a forbidden request-header"
        );
        assert_eq!(
            headers(&filled("PATCH")),
            vec![("x-http-method-override", "PATCH")],
            "`PATCH` is not, so the same name is allowed"
        );
    }
}
