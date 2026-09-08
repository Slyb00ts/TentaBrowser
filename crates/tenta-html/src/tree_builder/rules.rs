// Derived from html5ever's `src/tree_builder/rules.rs`.
// Copyright 2014-2017 The html5ever Project Developers, MIT/Apache-2.0.
// See THIRD_PARTY_NOTICES.md ("Derived code").

//! The insertion modes of the tree construction algorithm.

use std::borrow::Cow::Borrowed;

use tenta_core::atom::atoms as core_atoms;

use crate::encoding::extract_a_character_encoding_from_a_meta_element;
use crate::names::{ElemName, Local, Ns};
use crate::sink::{NodeOrText, QuirksMode, TreeSink};
use crate::tokenizer::states::RawKind::{Rawtext, Rcdata, ScriptData};
use crate::tokenizer::{Tag, TagKind};
use crate::tree_builder::tag_sets::*;
use crate::tree_builder::types::*;
use crate::tree_builder::{MAX_SCOPE_WALK, TreeBuilder};

/// A start-tag pattern with one or more local names.
macro_rules! start {
    ($($name:ident),+ $(,)?) => {
        Tag { kind: TagKind::StartTag, local: $(Local::$name)|+, .. }
    };
}

/// An end-tag pattern with one or more local names.
macro_rules! end {
    ($($name:ident),+ $(,)?) => {
        Tag { kind: TagKind::EndTag, local: $(Local::$name)|+, .. }
    };
}

/// Any start tag.
macro_rules! any_start {
    () => {
        Tag {
            kind: TagKind::StartTag,
            ..
        }
    };
}

/// Any end tag.
macro_rules! any_end {
    () => {
        Tag {
            kind: TagKind::EndTag,
            ..
        }
    };
}

fn any_not_whitespace(text: &str) -> bool {
    text.bytes().any(|b| !b.is_ascii_whitespace())
}

impl<Handle, Sink> TreeBuilder<Handle, Sink>
where
    Handle: Clone + Eq,
    Sink: TreeSink<Handle = Handle>,
{
    /// Processes one token in `mode`.
    pub(crate) fn step<'a>(
        &mut self,
        mode: InsertionMode,
        token: Token<'a>,
    ) -> ProcessResult<'a, Handle> {
        match mode {
            //§ the-initial-insertion-mode
            InsertionMode::Initial => match token {
                Token::Characters(SplitStatus::NotSplit, text) => {
                    ProcessResult::SplitWhitespace(text)
                }
                Token::Characters(SplitStatus::Whitespace, _) => ProcessResult::Done,
                Token::Comment(text) => {
                    self.append_comment_to_doc(text);
                    ProcessResult::Done
                }
                token => {
                    if !self.opts.iframe_srcdoc {
                        self.unexpected(&token);
                        self.set_quirks_mode(QuirksMode::Quirks);
                    }
                    ProcessResult::Reprocess(InsertionMode::BeforeHtml, token)
                }
            },

            //§ the-before-html-insertion-mode
            InsertionMode::BeforeHtml => match token {
                Token::Comment(text) => {
                    self.append_comment_to_doc(text);
                    ProcessResult::Done
                }
                Token::Characters(SplitStatus::NotSplit, text) => {
                    ProcessResult::SplitWhitespace(text)
                }
                Token::Characters(SplitStatus::Whitespace, _) => ProcessResult::Done,
                Token::Tag(tag @ start!(Html)) => {
                    self.create_root(tag.attrs);
                    self.mode = InsertionMode::BeforeHead;
                    ProcessResult::Done
                }
                Token::Tag(end!(Head, Body, Html, Br)) => {
                    self.create_root(Vec::new());
                    ProcessResult::Reprocess(InsertionMode::BeforeHead, token)
                }
                Token::Tag(any_end!()) => {
                    self.unexpected(&token);
                    ProcessResult::Done
                }
                token => {
                    self.create_root(Vec::new());
                    ProcessResult::Reprocess(InsertionMode::BeforeHead, token)
                }
            },

            //§ the-before-head-insertion-mode
            InsertionMode::BeforeHead => match token {
                Token::Characters(SplitStatus::NotSplit, text) => {
                    ProcessResult::SplitWhitespace(text)
                }
                Token::Characters(SplitStatus::Whitespace, _) => ProcessResult::Done,
                Token::Comment(text) => {
                    self.append_comment(text);
                    ProcessResult::Done
                }
                Token::Tag(start!(Html)) => self.step(InsertionMode::InBody, token),
                Token::Tag(tag @ start!(Head)) => {
                    self.head_elem = Some(self.insert_element_for(tag));
                    self.mode = InsertionMode::InHead;
                    ProcessResult::Done
                }
                Token::Tag(end!(Head, Body, Html, Br)) => {
                    let head = self.insert_phantom(Local::Head, core_atoms::HEAD);
                    self.head_elem = Some(head);
                    ProcessResult::Reprocess(InsertionMode::InHead, token)
                }
                Token::Tag(any_end!()) => {
                    self.unexpected(&token);
                    ProcessResult::Done
                }
                token => {
                    let head = self.insert_phantom(Local::Head, core_atoms::HEAD);
                    self.head_elem = Some(head);
                    ProcessResult::Reprocess(InsertionMode::InHead, token)
                }
            },

            //§ parsing-main-inhead
            InsertionMode::InHead => match token {
                Token::Characters(SplitStatus::NotSplit, text) => {
                    ProcessResult::SplitWhitespace(text)
                }
                Token::Characters(SplitStatus::Whitespace, text) => {
                    self.append_text(text);
                    ProcessResult::Done
                }
                Token::Comment(text) => {
                    self.append_comment(text);
                    ProcessResult::Done
                }

                Token::Tag(start!(Html)) => self.step(InsertionMode::InBody, token),

                Token::Tag(tag @ start!(Base, Basefont, Bgsound, Link, Meta)) => {
                    let charset = tag.attr(core_atoms::CHARSET).map(str::to_owned);
                    let http_equiv_content_type = tag
                        .attr(core_atoms::HTTP_EQUIV)
                        .is_some_and(|value| value.eq_ignore_ascii_case("content-type"));
                    let content = tag.attr(core_atoms::CONTENT).map(str::to_owned);
                    self.insert_and_pop_element_for(tag);

                    // Step 1 of the `<meta>` charset rules; the caller decides
                    // whether the label is usable and whether to re-parse.
                    if let Some(charset) = charset {
                        return ProcessResult::EncodingIndicator(charset);
                    }
                    if http_equiv_content_type
                        && let Some(content) = content
                        && let Some(label) =
                            extract_a_character_encoding_from_a_meta_element(&content)
                    {
                        return ProcessResult::EncodingIndicator(label);
                    }
                    ProcessResult::DoneAckSelfClosing
                }

                Token::Tag(tag @ start!(Title)) => self.parse_raw_data(tag, Rcdata),

                Token::Tag(tag @ start!(Noframes, Style, Noscript)) => {
                    if !self.opts.scripting_enabled && tag.local == Local::Noscript {
                        self.insert_element_for(tag);
                        self.mode = InsertionMode::InHeadNoscript;
                        ProcessResult::Done
                    } else {
                        self.parse_raw_data(tag, Rawtext)
                    }
                }

                Token::Tag(tag @ start!(Script)) => {
                    let name = ElemName::html(Local::Script, tag.name);
                    let attrs = tag.attrs;
                    let mut elem = self.create_element(name, attrs);
                    if self.is_fragment() {
                        self.sink.mark_script_already_started(&elem.handle);
                    }
                    let (point, depth) = self.appropriate_place_for_insertion(None);
                    self.insert_at(point, NodeOrText::Node(elem.handle.clone()));
                    elem.depth = depth;
                    elem.deepest = depth;
                    self.push_elem(elem);
                    self.switch_to_raw_text_mode(ScriptData)
                }

                Token::Tag(end!(Head)) => {
                    self.pop();
                    self.mode = InsertionMode::AfterHead;
                    ProcessResult::Done
                }

                Token::Tag(end!(Body, Html, Br)) => {
                    self.pop();
                    ProcessResult::Reprocess(InsertionMode::AfterHead, token)
                }

                Token::Tag(tag @ start!(Template)) => {
                    self.push_active_formatting(FormatEntry::Marker);
                    self.frameset_ok = false;
                    self.mode = InsertionMode::InTemplate;
                    self.template_modes.push(InsertionMode::InTemplate);
                    self.insert_element_for(tag);
                    ProcessResult::Done
                }

                Token::Tag(tag @ end!(Template)) => {
                    if !self.in_html_elem_named(Local::Template) {
                        self.unexpected(&tag);
                    } else {
                        self.generate_implied_end_tags(thorough_implied_end);
                        self.expect_to_close(Local::Template);
                        self.clear_active_formatting_to_marker();
                        self.template_modes.pop();
                        self.mode = self.reset_insertion_mode();
                    }
                    ProcessResult::Done
                }

                Token::Tag(start!(Head)) | Token::Tag(any_end!()) => {
                    self.unexpected(&token);
                    ProcessResult::Done
                }

                token => {
                    self.pop();
                    ProcessResult::Reprocess(InsertionMode::AfterHead, token)
                }
            },

            //§ parsing-main-inheadnoscript
            InsertionMode::InHeadNoscript => match token {
                Token::Tag(start!(Html)) => self.step(InsertionMode::InBody, token),
                Token::Tag(end!(Noscript)) => {
                    self.pop();
                    self.mode = InsertionMode::InHead;
                    ProcessResult::Done
                }
                Token::Characters(SplitStatus::NotSplit, text) => {
                    ProcessResult::SplitWhitespace(text)
                }
                Token::Characters(SplitStatus::Whitespace, _) => {
                    self.step(InsertionMode::InHead, token)
                }
                Token::Comment(_) => self.step(InsertionMode::InHead, token),
                Token::Tag(start!(Basefont, Bgsound, Link, Meta, Noframes, Style)) => {
                    self.step(InsertionMode::InHead, token)
                }
                Token::Tag(end!(Br)) => {
                    self.unexpected(&token);
                    self.pop();
                    ProcessResult::Reprocess(InsertionMode::InHead, token)
                }
                Token::Tag(start!(Head, Noscript)) | Token::Tag(any_end!()) => {
                    self.unexpected(&token);
                    ProcessResult::Done
                }
                token => {
                    self.unexpected(&token);
                    self.pop();
                    ProcessResult::Reprocess(InsertionMode::InHead, token)
                }
            },

            //§ the-after-head-insertion-mode
            InsertionMode::AfterHead => match token {
                Token::Characters(SplitStatus::NotSplit, text) => {
                    ProcessResult::SplitWhitespace(text)
                }
                Token::Characters(SplitStatus::Whitespace, text) => {
                    self.append_text(text);
                    ProcessResult::Done
                }
                Token::Comment(text) => {
                    self.append_comment(text);
                    ProcessResult::Done
                }

                Token::Tag(start!(Html)) => self.step(InsertionMode::InBody, token),

                Token::Tag(tag @ start!(Body)) => {
                    self.insert_element_for(tag);
                    self.frameset_ok = false;
                    self.mode = InsertionMode::InBody;
                    ProcessResult::Done
                }

                Token::Tag(tag @ start!(Frameset)) => {
                    self.insert_element_for(tag);
                    self.mode = InsertionMode::InFrameset;
                    ProcessResult::Done
                }

                Token::Tag(
                    start!(
                        Base, Basefont, Bgsound, Link, Meta, Noframes, Script, Style, Template,
                        Title
                    ),
                ) => {
                    self.unexpected(&token);
                    let head = self.head_elem.clone().expect("no head element");
                    let name = ElemName::html(Local::Head, core_atoms::HEAD);
                    self.push_elem(OpenElem {
                        handle: head.clone(),
                        name,
                        annotation_xml_integration_point: false,
                        // <head> is a child of <html>.
                        depth: 2,
                        deepest: 2,
                    });
                    let result = self.step(InsertionMode::InHead, token);
                    self.remove_from_stack(&head);
                    result
                }

                Token::Tag(end!(Template)) => self.step(InsertionMode::InHead, token),

                Token::Tag(end!(Body, Html, Br)) => {
                    self.insert_phantom(Local::Body, core_atoms::BODY);
                    ProcessResult::Reprocess(InsertionMode::InBody, token)
                }

                Token::Tag(start!(Head)) | Token::Tag(any_end!()) => {
                    self.unexpected(&token);
                    ProcessResult::Done
                }

                token => {
                    self.insert_phantom(Local::Body, core_atoms::BODY);
                    ProcessResult::Reprocess(InsertionMode::InBody, token)
                }
            },

            //§ parsing-main-inbody
            InsertionMode::InBody => self.step_in_body(token),

            //§ parsing-main-incdata
            InsertionMode::Text => match token {
                Token::Characters(_, text) => {
                    self.append_text(text);
                    ProcessResult::Done
                }

                Token::Eof => {
                    self.unexpected(&token);
                    if self.current_node_named(Local::Script) {
                        let current = self.current_handle();
                        self.sink.mark_script_already_started(&current);
                    }
                    self.pop();
                    let mode = self.orig_mode.take().expect("no original insertion mode");
                    ProcessResult::Reprocess(mode, token)
                }

                Token::Tag(tag @ any_end!()) => {
                    let node = self.pop();
                    self.mode = self.orig_mode.take().expect("no original insertion mode");
                    if tag.local == Local::Script {
                        return ProcessResult::Script(node);
                    }
                    ProcessResult::Done
                }

                _ => ProcessResult::Done,
            },

            //§ parsing-main-intable
            InsertionMode::InTable => match token {
                Token::NullCharacter | Token::Characters(..) => self.process_chars_in_table(token),

                Token::Comment(text) => {
                    self.append_comment(text);
                    ProcessResult::Done
                }

                Token::Tag(tag @ start!(Caption)) => {
                    self.pop_until_current(table_scope);
                    self.push_active_formatting(FormatEntry::Marker);
                    self.insert_element_for(tag);
                    self.mode = InsertionMode::InCaption;
                    ProcessResult::Done
                }

                Token::Tag(tag @ start!(Colgroup)) => {
                    self.pop_until_current(table_scope);
                    self.insert_element_for(tag);
                    self.mode = InsertionMode::InColumnGroup;
                    ProcessResult::Done
                }

                Token::Tag(start!(Col)) => {
                    self.pop_until_current(table_scope);
                    self.insert_phantom(Local::Colgroup, core_atoms::COLGROUP);
                    ProcessResult::Reprocess(InsertionMode::InColumnGroup, token)
                }

                Token::Tag(tag @ start!(Tbody, Tfoot, Thead)) => {
                    self.pop_until_current(table_scope);
                    self.insert_element_for(tag);
                    self.mode = InsertionMode::InTableBody;
                    ProcessResult::Done
                }

                Token::Tag(start!(Td, Th, Tr)) => {
                    self.pop_until_current(table_scope);
                    self.insert_phantom(Local::Tbody, core_atoms::TBODY);
                    ProcessResult::Reprocess(InsertionMode::InTableBody, token)
                }

                Token::Tag(start!(Table)) => {
                    self.unexpected(&token);
                    if self.in_scope_named(table_scope, Local::Table) {
                        self.pop_until_named(Local::Table);
                        let mode = self.reset_insertion_mode();
                        ProcessResult::Reprocess(mode, token)
                    } else {
                        ProcessResult::Done
                    }
                }

                Token::Tag(end!(Table)) => {
                    if self.in_scope_named(table_scope, Local::Table) {
                        self.pop_until_named(Local::Table);
                        self.mode = self.reset_insertion_mode();
                    } else {
                        self.unexpected(&token);
                    }
                    ProcessResult::Done
                }

                Token::Tag(
                    end!(
                        Body, Caption, Col, Colgroup, Html, Tbody, Td, Tfoot, Th, Thead, Tr
                    ),
                ) => {
                    self.unexpected(&token);
                    ProcessResult::Done
                }

                Token::Tag(start!(Style, Script, Template)) | Token::Tag(end!(Template)) => {
                    self.step(InsertionMode::InHead, token)
                }

                Token::Tag(tag @ start!(Input)) => {
                    self.unexpected(&tag);
                    if self.is_type_hidden(&tag) {
                        self.insert_and_pop_element_for(tag);
                        ProcessResult::DoneAckSelfClosing
                    } else {
                        self.foster_parent_in_body(Token::Tag(tag))
                    }
                }

                Token::Tag(tag @ start!(Form)) => {
                    self.unexpected(&tag);
                    if !self.in_html_elem_named(Local::Template) && self.form_elem.is_none() {
                        self.form_elem = Some(self.insert_and_pop_element_for(tag));
                    }
                    ProcessResult::Done
                }

                Token::Eof => self.step(InsertionMode::InBody, token),

                token => {
                    self.unexpected(&token);
                    self.foster_parent_in_body(token)
                }
            },

            //§ parsing-main-intabletext
            InsertionMode::InTableText => match token {
                Token::NullCharacter => {
                    self.unexpected(&token);
                    ProcessResult::Done
                }

                Token::Characters(split, text) => {
                    self.pending_table_text.push((split, text.to_owned()));
                    ProcessResult::Done
                }

                token => {
                    let pending = std::mem::take(&mut self.pending_table_text);
                    let has_non_space = pending.iter().any(|(split, text)| match split {
                        SplitStatus::Whitespace => false,
                        SplitStatus::NotWhitespace => true,
                        SplitStatus::NotSplit => any_not_whitespace(text),
                    });

                    if has_non_space {
                        self.parse_error(Borrowed("Non-space table text"));
                        for (split, text) in &pending {
                            match self.foster_parent_in_body(Token::Characters(*split, text)) {
                                ProcessResult::Done => (),
                                _ => unreachable!("unexpected result while foster parenting text"),
                            }
                        }
                    } else {
                        for (_, text) in &pending {
                            self.insert_appropriately(NodeOrText::Text(text), None);
                        }
                    }

                    let mode = self.orig_mode.take().expect("no original insertion mode");
                    ProcessResult::Reprocess(mode, token)
                }
            },

            //§ parsing-main-incaption
            InsertionMode::InCaption => match token {
                Token::Tag(
                    tag @ (start!(Caption, Col, Colgroup, Tbody, Td, Tfoot, Th, Thead, Tr)
                    | end!(Table, Caption)),
                ) => {
                    if self.in_scope_named(table_scope, Local::Caption) {
                        self.generate_implied_end_tags(cursory_implied_end);
                        self.expect_to_close(Local::Caption);
                        self.clear_active_formatting_to_marker();
                        if tag.kind == TagKind::EndTag && tag.local == Local::Caption {
                            self.mode = InsertionMode::InTable;
                            ProcessResult::Done
                        } else {
                            ProcessResult::Reprocess(InsertionMode::InTable, Token::Tag(tag))
                        }
                    } else {
                        self.unexpected(&tag);
                        ProcessResult::Done
                    }
                }

                Token::Tag(end!(Body, Col, Colgroup, Html, Tbody, Td, Tfoot, Th, Thead, Tr)) => {
                    self.unexpected(&token);
                    ProcessResult::Done
                }

                token => self.step(InsertionMode::InBody, token),
            },

            //§ parsing-main-incolgroup
            InsertionMode::InColumnGroup => match token {
                Token::Characters(SplitStatus::NotSplit, text) => {
                    ProcessResult::SplitWhitespace(text)
                }
                Token::Characters(SplitStatus::Whitespace, text) => {
                    self.append_text(text);
                    ProcessResult::Done
                }
                Token::Comment(text) => {
                    self.append_comment(text);
                    ProcessResult::Done
                }

                Token::Tag(start!(Html)) => self.step(InsertionMode::InBody, token),

                Token::Tag(tag @ start!(Col)) => {
                    self.insert_and_pop_element_for(tag);
                    ProcessResult::DoneAckSelfClosing
                }

                Token::Tag(end!(Colgroup)) => {
                    if self.current_node_named(Local::Colgroup) {
                        self.pop();
                        self.mode = InsertionMode::InTable;
                    } else {
                        self.unexpected(&token);
                    }
                    ProcessResult::Done
                }

                Token::Tag(end!(Col)) => {
                    self.unexpected(&token);
                    ProcessResult::Done
                }

                Token::Tag(start!(Template)) | Token::Tag(end!(Template)) => {
                    self.step(InsertionMode::InHead, token)
                }

                Token::Eof => self.step(InsertionMode::InBody, token),

                token => {
                    if self.current_node_named(Local::Colgroup) {
                        self.pop();
                        ProcessResult::Reprocess(InsertionMode::InTable, token)
                    } else {
                        self.unexpected(&token);
                        ProcessResult::Done
                    }
                }
            },

            //§ parsing-main-intbody
            InsertionMode::InTableBody => match token {
                Token::Tag(tag @ start!(Tr)) => {
                    self.pop_until_current(table_body_context);
                    self.insert_element_for(tag);
                    self.mode = InsertionMode::InRow;
                    ProcessResult::Done
                }

                Token::Tag(start!(Th, Td)) => {
                    self.unexpected(&token);
                    self.pop_until_current(table_body_context);
                    self.insert_phantom(Local::Tr, core_atoms::TR);
                    ProcessResult::Reprocess(InsertionMode::InRow, token)
                }

                Token::Tag(tag @ end!(Tbody, Tfoot, Thead)) => {
                    if self.in_scope_named(table_scope, tag.local) {
                        self.pop_until_current(table_body_context);
                        self.pop();
                        self.mode = InsertionMode::InTable;
                    } else {
                        self.unexpected(&tag);
                    }
                    ProcessResult::Done
                }

                Token::Tag(start!(Caption, Col, Colgroup, Tbody, Tfoot, Thead) | end!(Table)) => {
                    if self.in_scope(table_scope, |elem| {
                        elem.name.ns == Ns::Html
                            && matches!(elem.name.local, Local::Table | Local::Tbody | Local::Tfoot)
                    }) {
                        self.pop_until_current(table_body_context);
                        self.pop();
                        ProcessResult::Reprocess(InsertionMode::InTable, token)
                    } else {
                        self.unexpected(&token);
                        ProcessResult::Done
                    }
                }

                Token::Tag(end!(Body, Caption, Col, Colgroup, Html, Td, Th, Tr)) => {
                    self.unexpected(&token);
                    ProcessResult::Done
                }

                token => self.step(InsertionMode::InTable, token),
            },

            //§ parsing-main-intr
            InsertionMode::InRow => match token {
                Token::Tag(tag @ start!(Th, Td)) => {
                    self.pop_until_current(table_row_context);
                    self.insert_element_for(tag);
                    self.mode = InsertionMode::InCell;
                    self.push_active_formatting(FormatEntry::Marker);
                    ProcessResult::Done
                }

                Token::Tag(end!(Tr)) => {
                    if self.in_scope_named(table_scope, Local::Tr) {
                        self.pop_until_current(table_row_context);
                        self.pop();
                        self.mode = InsertionMode::InTableBody;
                    } else {
                        self.unexpected(&token);
                    }
                    ProcessResult::Done
                }

                Token::Tag(
                    start!(Caption, Col, Colgroup, Tbody, Tfoot, Thead, Tr) | end!(Table),
                ) => {
                    if self.in_scope_named(table_scope, Local::Tr) {
                        self.pop_until_current(table_row_context);
                        self.pop();
                        ProcessResult::Reprocess(InsertionMode::InTableBody, token)
                    } else {
                        self.unexpected(&token);
                        ProcessResult::Done
                    }
                }

                Token::Tag(tag @ end!(Tbody, Tfoot, Thead)) => {
                    if self.in_scope_named(table_scope, tag.local) {
                        if self.in_scope_named(table_scope, Local::Tr) {
                            self.pop_until_current(table_row_context);
                            self.pop();
                            ProcessResult::Reprocess(InsertionMode::InTableBody, Token::Tag(tag))
                        } else {
                            ProcessResult::Done
                        }
                    } else {
                        self.unexpected(&tag);
                        ProcessResult::Done
                    }
                }

                Token::Tag(end!(Body, Caption, Col, Colgroup, Html, Td, Th)) => {
                    self.unexpected(&token);
                    ProcessResult::Done
                }

                token => self.step(InsertionMode::InTable, token),
            },

            //§ parsing-main-intd
            InsertionMode::InCell => match token {
                Token::Tag(tag @ end!(Td, Th)) => {
                    if self.in_scope_named(table_scope, tag.local) {
                        self.generate_implied_end_tags(cursory_implied_end);
                        self.expect_to_close(tag.local);
                        self.clear_active_formatting_to_marker();
                        self.mode = InsertionMode::InRow;
                    } else {
                        self.unexpected(&tag);
                    }
                    ProcessResult::Done
                }

                Token::Tag(start!(Caption, Col, Colgroup, Tbody, Td, Tfoot, Th, Thead, Tr)) => {
                    if self.in_scope(table_scope, |elem| td_th(elem.name)) {
                        self.close_the_cell();
                        ProcessResult::Reprocess(InsertionMode::InRow, token)
                    } else {
                        self.unexpected(&token);
                        ProcessResult::Done
                    }
                }

                Token::Tag(end!(Body, Caption, Col, Colgroup, Html)) => {
                    self.unexpected(&token);
                    ProcessResult::Done
                }

                Token::Tag(tag @ end!(Table, Tbody, Tfoot, Thead, Tr)) => {
                    if self.in_scope_named(table_scope, tag.local) {
                        self.close_the_cell();
                        ProcessResult::Reprocess(InsertionMode::InRow, Token::Tag(tag))
                    } else {
                        self.unexpected(&tag);
                        ProcessResult::Done
                    }
                }

                token => self.step(InsertionMode::InBody, token),
            },

            //§ parsing-main-intemplate
            InsertionMode::InTemplate => match token {
                Token::Characters(..) | Token::Comment(_) => {
                    self.step(InsertionMode::InBody, token)
                }

                Token::Tag(
                    start!(
                        Base, Basefont, Bgsound, Link, Meta, Noframes, Script, Style, Template,
                        Title
                    ),
                )
                | Token::Tag(end!(Template)) => self.step(InsertionMode::InHead, token),

                Token::Tag(start!(Caption, Colgroup, Tbody, Tfoot, Thead)) => {
                    self.template_modes.pop();
                    self.template_modes.push(InsertionMode::InTable);
                    ProcessResult::Reprocess(InsertionMode::InTable, token)
                }

                Token::Tag(start!(Col)) => {
                    self.template_modes.pop();
                    self.template_modes.push(InsertionMode::InColumnGroup);
                    ProcessResult::Reprocess(InsertionMode::InColumnGroup, token)
                }

                Token::Tag(start!(Tr)) => {
                    self.template_modes.pop();
                    self.template_modes.push(InsertionMode::InTableBody);
                    ProcessResult::Reprocess(InsertionMode::InTableBody, token)
                }

                Token::Tag(start!(Td, Th)) => {
                    self.template_modes.pop();
                    self.template_modes.push(InsertionMode::InRow);
                    ProcessResult::Reprocess(InsertionMode::InRow, token)
                }

                Token::Eof => {
                    if !self.in_html_elem_named(Local::Template) {
                        ProcessResult::Done
                    } else {
                        self.unexpected(&token);
                        self.pop_until_named(Local::Template);
                        self.clear_active_formatting_to_marker();
                        self.template_modes.pop();
                        let mode = self.reset_insertion_mode();
                        self.mode = mode;
                        ProcessResult::Reprocess(mode, token)
                    }
                }

                Token::Tag(tag @ any_start!()) => {
                    self.template_modes.pop();
                    self.template_modes.push(InsertionMode::InBody);
                    ProcessResult::Reprocess(InsertionMode::InBody, Token::Tag(tag))
                }

                token => {
                    self.unexpected(&token);
                    ProcessResult::Done
                }
            },

            //§ parsing-main-afterbody
            InsertionMode::AfterBody => match token {
                Token::Characters(SplitStatus::NotSplit, text) => {
                    ProcessResult::SplitWhitespace(text)
                }
                Token::Characters(SplitStatus::Whitespace, _) => {
                    self.step(InsertionMode::InBody, token)
                }
                Token::Comment(text) => {
                    self.append_comment_to_html(text);
                    ProcessResult::Done
                }

                Token::Tag(start!(Html)) => self.step(InsertionMode::InBody, token),

                Token::Tag(end!(Html)) => {
                    if self.is_fragment() {
                        self.unexpected(&token);
                    } else {
                        self.mode = InsertionMode::AfterAfterBody;
                    }
                    ProcessResult::Done
                }

                Token::Eof => ProcessResult::Done,

                token => {
                    self.unexpected(&token);
                    ProcessResult::Reprocess(InsertionMode::InBody, token)
                }
            },

            //§ parsing-main-inframeset
            InsertionMode::InFrameset => match token {
                Token::Characters(SplitStatus::NotSplit, text) => {
                    ProcessResult::SplitWhitespace(text)
                }
                Token::Characters(SplitStatus::Whitespace, text) => {
                    self.append_text(text);
                    ProcessResult::Done
                }
                Token::Comment(text) => {
                    self.append_comment(text);
                    ProcessResult::Done
                }

                Token::Tag(start!(Html)) => self.step(InsertionMode::InBody, token),

                Token::Tag(tag @ start!(Frameset)) => {
                    self.insert_element_for(tag);
                    ProcessResult::Done
                }

                Token::Tag(end!(Frameset)) => {
                    if self.open_elems.len() == 1 {
                        self.unexpected(&token);
                    } else {
                        self.pop();
                        if !self.is_fragment() && !self.current_node_named(Local::Frameset) {
                            self.mode = InsertionMode::AfterFrameset;
                        }
                    }
                    ProcessResult::Done
                }

                Token::Tag(tag @ start!(Frame)) => {
                    self.insert_and_pop_element_for(tag);
                    ProcessResult::DoneAckSelfClosing
                }

                Token::Tag(start!(Noframes)) => self.step(InsertionMode::InHead, token),

                Token::Eof => {
                    if self.open_elems.len() != 1 {
                        self.unexpected(&token);
                    }
                    ProcessResult::Done
                }

                token => {
                    self.unexpected(&token);
                    ProcessResult::Done
                }
            },

            //§ parsing-main-afterframeset
            InsertionMode::AfterFrameset => match token {
                Token::Characters(SplitStatus::NotSplit, text) => {
                    ProcessResult::SplitWhitespace(text)
                }
                Token::Characters(SplitStatus::Whitespace, text) => {
                    self.append_text(text);
                    ProcessResult::Done
                }
                Token::Comment(text) => {
                    self.append_comment(text);
                    ProcessResult::Done
                }

                Token::Tag(start!(Html)) => self.step(InsertionMode::InBody, token),

                Token::Tag(end!(Html)) => {
                    self.mode = InsertionMode::AfterAfterFrameset;
                    ProcessResult::Done
                }

                Token::Tag(start!(Noframes)) => self.step(InsertionMode::InHead, token),

                Token::Eof => ProcessResult::Done,

                token => {
                    self.unexpected(&token);
                    ProcessResult::Done
                }
            },

            //§ the-after-after-body-insertion-mode
            InsertionMode::AfterAfterBody => match token {
                Token::Characters(SplitStatus::NotSplit, text) => {
                    ProcessResult::SplitWhitespace(text)
                }
                Token::Characters(SplitStatus::Whitespace, _) => {
                    self.step(InsertionMode::InBody, token)
                }
                Token::Comment(text) => {
                    self.append_comment_to_doc(text);
                    ProcessResult::Done
                }
                Token::Tag(start!(Html)) => self.step(InsertionMode::InBody, token),
                Token::Eof => ProcessResult::Done,
                token => {
                    self.unexpected(&token);
                    ProcessResult::Reprocess(InsertionMode::InBody, token)
                }
            },

            //§ the-after-after-frameset-insertion-mode
            InsertionMode::AfterAfterFrameset => match token {
                Token::Characters(SplitStatus::NotSplit, text) => {
                    ProcessResult::SplitWhitespace(text)
                }
                Token::Characters(SplitStatus::Whitespace, _) => {
                    self.step(InsertionMode::InBody, token)
                }
                Token::Comment(text) => {
                    self.append_comment_to_doc(text);
                    ProcessResult::Done
                }
                Token::Tag(start!(Html)) => self.step(InsertionMode::InBody, token),
                Token::Eof => ProcessResult::Done,
                Token::Tag(start!(Noframes)) => self.step(InsertionMode::InHead, token),
                token => {
                    self.unexpected(&token);
                    ProcessResult::Done
                }
            },
        }
    }
}

impl<Handle, Sink> TreeBuilder<Handle, Sink>
where
    Handle: Clone + Eq,
    Sink: TreeSink<Handle = Handle>,
{
    /// § The "in body" insertion mode.
    fn step_in_body<'a>(&mut self, token: Token<'a>) -> ProcessResult<'a, Handle> {
        match token {
            Token::NullCharacter => {
                self.unexpected(&token);
                ProcessResult::Done
            }

            Token::Characters(_, text) => {
                self.reconstruct_active_formatting_elements();
                if any_not_whitespace(text) {
                    self.frameset_ok = false;
                }
                self.append_text(text);
                ProcessResult::Done
            }

            Token::Comment(text) => {
                self.append_comment(text);
                ProcessResult::Done
            }

            Token::Tag(tag @ start!(Html)) => {
                self.unexpected(&tag);
                if !self.in_html_elem_named(Local::Template) {
                    let top = self.open_elems[0].handle.clone();
                    self.sink.add_attrs_if_missing(&top, tag.attrs);
                }
                ProcessResult::Done
            }

            Token::Tag(
                start!(
                    Base, Basefont, Bgsound, Link, Meta, Noframes, Script, Style, Template, Title
                ),
            )
            | Token::Tag(end!(Template)) => self.step(InsertionMode::InHead, token),

            Token::Tag(tag @ start!(Body)) => {
                self.unexpected(&tag);
                let body = self.body_elem();
                if let Some(body) = body
                    && self.open_elems.len() != 1
                    && !self.in_html_elem_named(Local::Template)
                {
                    self.frameset_ok = false;
                    self.sink.add_attrs_if_missing(&body, tag.attrs);
                }
                ProcessResult::Done
            }

            Token::Tag(tag @ start!(Frameset)) => {
                self.unexpected(&tag);
                if !self.frameset_ok {
                    return ProcessResult::Done;
                }
                let Some(body) = self.body_elem() else {
                    return ProcessResult::Done;
                };
                self.sink.remove_from_parent(&body);
                self.truncate_stack(1);
                self.insert_element_for(tag);
                self.mode = InsertionMode::InFrameset;
                ProcessResult::Done
            }

            Token::Eof => {
                if !self.template_modes.is_empty() {
                    self.step(InsertionMode::InTemplate, token)
                } else {
                    self.check_body_end();
                    ProcessResult::Done
                }
            }

            Token::Tag(end!(Body)) => {
                if self.in_scope_named(default_scope, Local::Body) {
                    self.check_body_end();
                    self.mode = InsertionMode::AfterBody;
                } else {
                    self.parse_error(Borrowed("</body> with no <body> in scope"));
                }
                ProcessResult::Done
            }

            Token::Tag(end!(Html)) => {
                if self.in_scope_named(default_scope, Local::Body) {
                    self.check_body_end();
                    ProcessResult::Reprocess(InsertionMode::AfterBody, token)
                } else {
                    self.parse_error(Borrowed("</html> with no <body> in scope"));
                    ProcessResult::Done
                }
            }

            Token::Tag(
                tag @ start!(
                    Address, Article, Aside, Blockquote, Center, Details, Dialog, Dir, Div, Dl,
                    Fieldset, Figcaption, Figure, Footer, Header, Hgroup, Main, Menu, Nav, Ol, P,
                    Search, Section, Summary, Ul
                ),
            ) => {
                self.close_p_element_in_button_scope();
                self.insert_element_for(tag);
                ProcessResult::Done
            }

            Token::Tag(tag @ start!(H1, H2, H3, H4, H5, H6)) => {
                self.close_p_element_in_button_scope();
                if self.current_node_in(heading_tag) {
                    self.parse_error(Borrowed("nested heading tags"));
                    self.pop();
                }
                self.insert_element_for(tag);
                ProcessResult::Done
            }

            Token::Tag(tag @ start!(Pre, Listing)) => {
                self.close_p_element_in_button_scope();
                self.insert_element_for(tag);
                self.ignore_lf = true;
                self.frameset_ok = false;
                ProcessResult::Done
            }

            Token::Tag(tag @ start!(Form)) => {
                if self.form_elem.is_some() && !self.in_html_elem_named(Local::Template) {
                    self.parse_error(Borrowed("nested forms"));
                } else {
                    self.close_p_element_in_button_scope();
                    let elem = self.insert_element_for(tag);
                    if !self.in_html_elem_named(Local::Template) {
                        self.form_elem = Some(elem);
                    }
                }
                ProcessResult::Done
            }

            Token::Tag(tag @ start!(Li, Dd, Dt)) => {
                let list = tag.local == Local::Li;
                self.frameset_ok = false;

                // Bound the scan to the top `MAX_SCOPE_WALK` entries (owner Q5):
                // a list item deeper than that on an adversarially deep stack is
                // treated as not present, which keeps this rule linear instead of
                // quadratic on `<li>`/`<dd>`/`<dt>` floods (N5-1).
                let mut to_close = None;
                for elem in self.open_elems.iter().rev().take(MAX_SCOPE_WALK) {
                    let name = elem.name;
                    let can_close = if list {
                        name.is_html(Local::Li)
                    } else {
                        name.ns == Ns::Html && matches!(name.local, Local::Dd | Local::Dt)
                    };
                    if can_close {
                        to_close = Some(name.local);
                        break;
                    }
                    // "special", minus address, div and p.
                    if special_tag(name)
                        && !matches!(name.local, Local::Address | Local::Div | Local::P)
                    {
                        break;
                    }
                }

                if let Some(local) = to_close {
                    self.generate_implied_end_except(local);
                    self.expect_to_close(local);
                }

                self.close_p_element_in_button_scope();
                self.insert_element_for(tag);
                ProcessResult::Done
            }

            Token::Tag(tag @ start!(Plaintext)) => {
                self.close_p_element_in_button_scope();
                self.insert_element_for(tag);
                ProcessResult::ToPlaintext
            }

            Token::Tag(tag @ start!(Button)) => {
                if self.in_scope_named(default_scope, Local::Button) {
                    self.parse_error(Borrowed("nested buttons"));
                    self.generate_implied_end_tags(cursory_implied_end);
                    self.pop_until_named(Local::Button);
                }
                self.reconstruct_active_formatting_elements();
                self.insert_element_for(tag);
                self.frameset_ok = false;
                ProcessResult::Done
            }

            Token::Tag(
                tag @ end!(
                    Address, Article, Aside, Blockquote, Button, Center, Details, Dialog, Dir, Div,
                    Dl, Fieldset, Figcaption, Figure, Footer, Header, Hgroup, Listing, Main, Menu,
                    Nav, Ol, Pre, Search, Section, Select, Summary, Ul
                ),
            ) => {
                if !self.in_scope_named(default_scope, tag.local) {
                    self.unexpected(&tag);
                } else {
                    self.generate_implied_end_tags(cursory_implied_end);
                    self.expect_to_close(tag.local);
                }
                ProcessResult::Done
            }

            Token::Tag(end!(Form)) => {
                if !self.in_html_elem_named(Local::Template) {
                    let Some(node) = self.form_elem.take() else {
                        self.parse_error(Borrowed("Null form element pointer on </form>"));
                        return ProcessResult::Done;
                    };
                    if !self.in_scope_handle(default_scope, &node) {
                        self.parse_error(Borrowed("Form element not in scope on </form>"));
                        return ProcessResult::Done;
                    }
                    self.generate_implied_end_tags(cursory_implied_end);
                    let current = self.current_handle();
                    self.remove_from_stack(&node);
                    if !self.sink.same_node(&current, &node) {
                        self.parse_error(Borrowed("Bad open element on </form>"));
                    }
                } else {
                    if !self.in_scope_named(default_scope, Local::Form) {
                        self.parse_error(Borrowed("Form element not in scope on </form>"));
                        return ProcessResult::Done;
                    }
                    self.generate_implied_end_tags(cursory_implied_end);
                    if !self.current_node_named(Local::Form) {
                        self.parse_error(Borrowed("Bad open element on </form>"));
                    }
                    self.pop_until_named(Local::Form);
                }
                ProcessResult::Done
            }

            Token::Tag(end!(P)) => {
                if !self.in_scope_named(button_scope, Local::P) {
                    self.parse_error(Borrowed("No <p> tag to close"));
                    self.insert_phantom(Local::P, core_atoms::P);
                }
                self.close_p_element();
                ProcessResult::Done
            }

            Token::Tag(tag @ end!(Li, Dd, Dt)) => {
                let in_scope = if tag.local == Local::Li {
                    self.in_scope_named(list_item_scope, tag.local)
                } else {
                    self.in_scope_named(default_scope, tag.local)
                };
                if in_scope {
                    self.generate_implied_end_except(tag.local);
                    self.expect_to_close(tag.local);
                } else {
                    self.parse_error(Borrowed("No matching tag to close"));
                }
                ProcessResult::Done
            }

            Token::Tag(tag @ end!(H1, H2, H3, H4, H5, H6)) => {
                if self.in_scope(default_scope, |elem| heading_tag(elem.name)) {
                    self.generate_implied_end_tags(cursory_implied_end);
                    if !self.current_node_named(tag.local) {
                        self.parse_error(Borrowed("Closing wrong heading tag"));
                    }
                    self.pop_until(heading_tag);
                } else {
                    self.parse_error(Borrowed("No heading tag to close"));
                }
                ProcessResult::Done
            }

            Token::Tag(tag @ start!(A)) => {
                self.handle_misnested_a_tags(&tag);
                self.reconstruct_active_formatting_elements();
                self.create_formatting_element_for(tag);
                ProcessResult::Done
            }

            Token::Tag(
                tag @ start!(B, Big, Code, Em, Font, I, S, Small, Strike, Strong, Tt, U),
            ) => {
                self.reconstruct_active_formatting_elements();
                self.create_formatting_element_for(tag);
                ProcessResult::Done
            }

            Token::Tag(tag @ start!(Nobr)) => {
                self.reconstruct_active_formatting_elements();
                if self.in_scope_named(default_scope, Local::Nobr) {
                    self.parse_error(Borrowed("Nested <nobr>"));
                    self.adoption_agency(tag.name, Local::Nobr);
                    self.reconstruct_active_formatting_elements();
                }
                self.create_formatting_element_for(tag);
                ProcessResult::Done
            }

            Token::Tag(
                tag @ end!(
                    A, B, Big, Code, Em, Font, I, Nobr, S, Small, Strike, Strong, Tt, U
                ),
            ) => {
                self.adoption_agency(tag.name, tag.local);
                ProcessResult::Done
            }

            Token::Tag(tag @ start!(Applet, Marquee, Object)) => {
                self.reconstruct_active_formatting_elements();
                self.insert_element_for(tag);
                self.push_active_formatting(FormatEntry::Marker);
                self.frameset_ok = false;
                ProcessResult::Done
            }

            Token::Tag(tag @ end!(Applet, Marquee, Object)) => {
                if !self.in_scope_named(default_scope, tag.local) {
                    self.unexpected(&tag);
                } else {
                    self.generate_implied_end_tags(cursory_implied_end);
                    self.expect_to_close(tag.local);
                    self.clear_active_formatting_to_marker();
                }
                ProcessResult::Done
            }

            Token::Tag(tag @ start!(Table)) => {
                if self.quirks_mode != QuirksMode::Quirks {
                    self.close_p_element_in_button_scope();
                }
                self.insert_element_for(tag);
                self.frameset_ok = false;
                self.mode = InsertionMode::InTable;
                ProcessResult::Done
            }

            Token::Tag(tag @ end!(Br)) => {
                self.unexpected(&tag);
                self.step_in_body(Token::Tag(Tag {
                    kind: TagKind::StartTag,
                    attrs: Vec::new(),
                    ..tag
                }))
            }

            Token::Tag(tag @ start!(Area, Br, Embed, Img, Keygen, Wbr)) => {
                self.reconstruct_active_formatting_elements();
                self.insert_and_pop_element_for(tag);
                self.frameset_ok = false;
                ProcessResult::DoneAckSelfClosing
            }

            Token::Tag(tag @ start!(Input)) => {
                // A fragment parse whose context is <select> has no select on
                // the stack to close, so the token is dropped
                // (html5lib-tests tests_innerHTML_1.dat: "<input><option>").
                if self.is_fragment()
                    && self
                        .context_elem
                        .as_ref()
                        .is_some_and(|c| c.name.is_html(Local::Select))
                {
                    self.unexpected(&tag);
                    return ProcessResult::Done;
                }
                if self.in_scope_named(default_scope, Local::Select) {
                    self.unexpected(&tag);
                    self.pop_until_named(Local::Select);
                }
                let is_type_hidden = self.is_type_hidden(&tag);
                self.reconstruct_active_formatting_elements();
                self.insert_and_pop_element_for(tag);
                if !is_type_hidden {
                    self.frameset_ok = false;
                }
                ProcessResult::DoneAckSelfClosing
            }

            Token::Tag(tag @ start!(Param, Source, Track)) => {
                self.insert_and_pop_element_for(tag);
                ProcessResult::DoneAckSelfClosing
            }

            Token::Tag(tag @ start!(Hr)) => {
                self.close_p_element_in_button_scope();
                if self.in_scope_named(default_scope, Local::Select) {
                    self.generate_implied_end_tags(cursory_implied_end);
                    if self.in_scope_named(default_scope, Local::Option)
                        || self.in_scope_named(default_scope, Local::Optgroup)
                    {
                        self.parse_error(Borrowed("hr in option"));
                    }
                }
                self.insert_and_pop_element_for(tag);
                self.frameset_ok = false;
                ProcessResult::DoneAckSelfClosing
            }

            Token::Tag(tag @ start!(Image)) => {
                self.unexpected(&tag);
                self.step_in_body(Token::Tag(Tag {
                    name: core_atoms::IMG,
                    local: Local::Img,
                    ..tag
                }))
            }

            Token::Tag(tag @ start!(Textarea)) => {
                self.ignore_lf = true;
                self.frameset_ok = false;
                self.parse_raw_data(tag, Rcdata)
            }

            Token::Tag(tag @ start!(Xmp)) => {
                self.close_p_element_in_button_scope();
                self.reconstruct_active_formatting_elements();
                self.frameset_ok = false;
                self.parse_raw_data(tag, Rawtext)
            }

            Token::Tag(tag @ start!(Iframe)) => {
                self.frameset_ok = false;
                self.parse_raw_data(tag, Rawtext)
            }

            Token::Tag(tag @ start!(Noembed)) => self.parse_raw_data(tag, Rawtext),

            Token::Tag(tag @ start!(Select)) => {
                if self.is_fragment()
                    && self
                        .context_elem
                        .as_ref()
                        .is_some_and(|c| c.name.is_html(Local::Select))
                {
                    self.unexpected(&tag);
                } else if self.in_scope_named(default_scope, Local::Select) {
                    self.unexpected(&tag);
                    self.pop_until_named(Local::Select);
                } else {
                    self.reconstruct_active_formatting_elements();
                    self.insert_element_for(tag);
                    self.frameset_ok = false;
                }
                ProcessResult::Done
            }

            Token::Tag(tag @ start!(Option)) => {
                if self.in_scope_named(default_scope, Local::Select) {
                    self.generate_implied_end_except(Local::Optgroup);
                    if self.in_scope_named(default_scope, Local::Option) {
                        self.parse_error(Borrowed("nested options"));
                    }
                } else if self.current_node_named(Local::Option) {
                    self.pop();
                }
                self.reconstruct_active_formatting_elements();
                self.insert_element_for(tag);
                ProcessResult::Done
            }

            Token::Tag(tag @ start!(Optgroup)) => {
                if self.in_scope_named(default_scope, Local::Select) {
                    self.generate_implied_end_tags(cursory_implied_end);
                    if self.in_scope_named(default_scope, Local::Option)
                        || self.in_scope_named(default_scope, Local::Optgroup)
                    {
                        self.parse_error(Borrowed("nested options"));
                    }
                } else if self.current_node_named(Local::Option) {
                    self.pop();
                }
                self.reconstruct_active_formatting_elements();
                self.insert_element_for(tag);
                ProcessResult::Done
            }

            Token::Tag(tag @ start!(Rb, Rtc)) => {
                if self.in_scope_named(default_scope, Local::Ruby) {
                    self.generate_implied_end_tags(cursory_implied_end);
                }
                if !self.current_node_named(Local::Ruby) {
                    self.unexpected(&tag);
                }
                self.insert_element_for(tag);
                ProcessResult::Done
            }

            Token::Tag(tag @ start!(Rp, Rt)) => {
                if self.in_scope_named(default_scope, Local::Ruby) {
                    self.generate_implied_end_except(Local::Rtc);
                }
                if !self.current_node_named(Local::Rtc) && !self.current_node_named(Local::Ruby) {
                    self.unexpected(&tag);
                }
                self.insert_element_for(tag);
                ProcessResult::Done
            }

            Token::Tag(tag @ start!(Math)) => {
                self.reconstruct_active_formatting_elements();
                self.enter_foreign(tag, Ns::MathMl)
            }

            Token::Tag(tag @ start!(Svg)) => {
                self.reconstruct_active_formatting_elements();
                self.enter_foreign(tag, Ns::Svg)
            }

            Token::Tag(
                start!(
                    Caption, Col, Colgroup, Frame, Head, Tbody, Td, Tfoot, Th, Thead, Tr
                ),
            ) => {
                self.unexpected(&token);
                ProcessResult::Done
            }

            Token::Tag(tag @ any_start!()) => {
                if self.opts.scripting_enabled && tag.local == Local::Noscript {
                    self.parse_raw_data(tag, Rawtext)
                } else {
                    self.reconstruct_active_formatting_elements();
                    self.insert_element_for(tag);
                    ProcessResult::Done
                }
            }

            Token::Tag(tag @ any_end!()) => {
                self.process_end_tag_in_body(tag);
                ProcessResult::Done
            }
        }
    }

    /// The second element on the stack, if it is a `<body>`.
    fn body_elem(&self) -> Option<Handle> {
        if self.open_elems.len() <= 1 {
            return None;
        }
        let elem = &self.open_elems[1];
        if elem.name.is_html(Local::Body) {
            Some(elem.handle.clone())
        } else {
            None
        }
    }

    /// § The rules for parsing tokens in foreign content.
    pub(crate) fn step_foreign<'a>(&mut self, token: Token<'a>) -> ProcessResult<'a, Handle> {
        match token {
            Token::NullCharacter => {
                self.unexpected(&token);
                self.append_text("\u{fffd}");
                ProcessResult::Done
            }

            Token::Characters(_, text) => {
                if any_not_whitespace(text) {
                    self.frameset_ok = false;
                }
                self.append_text(text);
                ProcessResult::Done
            }

            Token::Comment(text) => {
                self.append_comment(text);
                ProcessResult::Done
            }

            Token::Tag(
                tag @ (start!(
                    B, Big, Blockquote, Body, Br, Center, Code, Dd, Div, Dl, Dt, Em, Embed, H1, H2,
                    H3, H4, H5, H6, Head, Hr, I, Img, Li, Listing, Menu, Meta, Nobr, Ol, P, Pre,
                    Ruby, S, Small, Span, Strong, Strike, Sub, Sup, Table, Tt, U, Ul, Var
                )
                | end!(Br, P)),
            ) => self.unexpected_start_tag_in_foreign_content(tag),

            Token::Tag(tag @ start!(Font)) => {
                let unexpected = tag.attrs.iter().any(|attr| {
                    attr.name.prefix.is_none()
                        && matches!(attr.name.local.as_str(), "color" | "face" | "size")
                });
                if unexpected {
                    self.unexpected_start_tag_in_foreign_content(tag)
                } else {
                    self.foreign_start_tag(tag)
                }
            }

            Token::Tag(tag @ any_start!()) => self.foreign_start_tag(tag),

            Token::Tag(tag @ any_end!()) => {
                let mut first = true;
                let mut index = self.open_elems.len() - 1;
                let search_limit = self.open_elems.len().saturating_sub(MAX_SCOPE_WALK);
                loop {
                    if index < search_limit || index == 0 {
                        // Cap reached or no more elements: treat as unmatched.
                        return ProcessResult::Done;
                    }
                    let name = self.open_elems[index].name;
                    let is_html = name.ns == Ns::Html;
                    let matches = name.atom.as_str().eq_ignore_ascii_case(tag.name.as_str());
                    if !first && is_html {
                        let mode = self.mode;
                        return self.step(mode, Token::Tag(tag));
                    }
                    if matches {
                        self.truncate_stack(index);
                        return ProcessResult::Done;
                    }
                    if first {
                        self.unexpected(&tag);
                        first = false;
                    }
                    index -= 1;
                }
            }

            Token::Eof => ProcessResult::Done,
        }
    }
}
