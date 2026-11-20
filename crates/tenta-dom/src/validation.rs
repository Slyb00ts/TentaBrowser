//! Constraint validation of form controls
//! (<https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#constraints>).
//!
//! What a control holds is read through [`crate::forms`], so validity agrees
//! with what a submission would send. The script bindings turn the answers
//! into `ValidityState`, `checkValidity()` and the refusal of an invalid
//! submission; the extractors can read the same answers.
//!
//! Departures from the standard, each because the information is not kept:
//!
//! - `tooShort` is reported for any dirty value, not only one the user
//!   typed: the agent's typing and a script's assignment both raise the
//!   dirty value flag and nothing tells them apart. `tooLong` is never
//!   reported, which is what a browser shows, since typing past `maxlength`
//!   is refused and an assignment does not count.
//! - `pattern` is compiled by `regex-lite`, not by the page's script engine:
//!   the common syntax (classes, quantifiers, groups, alternation, `\d`,
//!   `\w`) means the same, and a pattern it cannot compile (lookaround,
//!   backreferences, `\p{..}`) is ignored, as an invalid one is.
//! - A value is not sanitized when it is set, so a `number` or date control
//!   holding text that does not parse reports `badInput` where a browser
//!   would have emptied the value of an assignment.
//! - A `file` control has no selected files here, so a required one is
//!   always missing its value.

use tenta_core::NodeId;
use tenta_core::atom::atoms;

use crate::document::Document;
use crate::forms;

/// The ways a control can fail its constraints: the members of
/// `ValidityState` but `valid`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Validity {
    pub value_missing: bool,
    pub type_mismatch: bool,
    pub pattern_mismatch: bool,
    pub too_long: bool,
    pub too_short: bool,
    pub range_underflow: bool,
    pub range_overflow: bool,
    pub step_mismatch: bool,
    pub bad_input: bool,
    pub custom_error: bool,
}

impl Validity {
    /// Whether the control satisfies its constraints.
    pub fn valid(&self) -> bool {
        *self == Self::default()
    }
}

fn has(doc: &Document, node: NodeId, name: &str) -> bool {
    doc.attribute_str(node, name).is_some()
}

/// The input types the `readonly` attribute applies to.
fn readonly_applies(kind: &str) -> bool {
    matches!(
        kind,
        "text"
            | "search"
            | "url"
            | "tel"
            | "email"
            | "password"
            | "date"
            | "month"
            | "week"
            | "time"
            | "datetime-local"
            | "number"
    )
}

/// The input types whose value is text the `pattern`, `minlength` and
/// `maxlength` attributes apply to.
fn text_like(kind: &str) -> bool {
    matches!(
        kind,
        "text" | "search" | "url" | "tel" | "email" | "password"
    )
}

/// Whether the element is a submittable element the constraint validation
/// API is defined on: `<input>`, `<select>`, `<textarea>` and `<button>`.
pub fn is_validatable(doc: &Document, node: NodeId) -> bool {
    matches!(
        doc.tag(node),
        Some(tag)
            if tag == atoms::INPUT
                || tag == atoms::SELECT
                || tag == atoms::TEXTAREA
                || tag == atoms::BUTTON
    )
}

/// `willValidate`: the control is a candidate for constraint validation,
/// that is, not barred by its type, by being disabled or read-only, or by
/// sitting in a `<datalist>`.
pub fn will_validate(doc: &Document, control: NodeId) -> bool {
    let Some(tag) = doc.tag(control) else {
        return false;
    };
    if tag == atoms::INPUT {
        let kind = forms::input_type(doc, control);
        if matches!(kind, "hidden" | "reset" | "button") {
            return false;
        }
        if readonly_applies(kind) && has(doc, control, "readonly") {
            return false;
        }
    } else if tag == atoms::BUTTON {
        if forms::button_type(doc, control) != "submit" {
            return false;
        }
    } else if tag == atoms::TEXTAREA {
        if has(doc, control, "readonly") {
            return false;
        }
    } else if tag != atoms::SELECT {
        return false;
    }
    if forms::is_disabled(doc, control) {
        return false;
    }
    !doc.ancestors(control)
        .any(|ancestor| doc.tag(ancestor) == Some(atoms::DATALIST))
}

/// The validity of `control`. A control that is not a candidate for
/// constraint validation answers with no failures but `customError`, which
/// the standard reports whatever the candidacy.
pub fn validity(doc: &Document, control: NodeId) -> Validity {
    let mut validity = Validity {
        custom_error: doc.custom_validity(control).is_some(),
        ..Validity::default()
    };
    if !will_validate(doc, control) {
        return validity;
    }
    match doc.tag(control) {
        Some(tag) if tag == atoms::INPUT => input_validity(doc, control, &mut validity),
        Some(tag) if tag == atoms::TEXTAREA => {
            let value = forms::control_value(doc, control);
            validity.value_missing = has(doc, control, "required") && value.is_empty();
            validity.too_short = too_short(doc, control, &value);
        }
        Some(tag) if tag == atoms::SELECT => {
            validity.value_missing = has(doc, control, "required") && select_missing(doc, control);
        }
        _ => {}
    }
    validity
}

/// Whether a required `<select>` has no value: nothing is selected, or only
/// its placeholder label option is.
fn select_missing(doc: &Document, select: NodeId) -> bool {
    let selected = forms::selected_options(doc, select);
    if selected.is_empty() {
        return true;
    }
    if has(doc, select, "multiple") || forms::display_size(doc, select) != 1 {
        return false;
    }
    forms::select_options(doc, select)
        .first()
        .is_some_and(|first| {
            doc.parent(*first) == select
                && forms::option_value(doc, *first).is_empty()
                && selected == [*first]
        })
}

/// `tooShort` for a text value: a dirty, non-empty value shorter than
/// `minlength` in UTF-16 code units.
fn too_short(doc: &Document, control: NodeId, value: &str) -> bool {
    if value.is_empty() || doc.dirty_value(control).is_none() {
        return false;
    }
    let Some(min) = doc
        .attribute_str(control, "minlength")
        .and_then(|min| min.trim().parse::<usize>().ok())
    else {
        return false;
    };
    value.encode_utf16().count() < min
}

fn input_validity(doc: &Document, input: NodeId, validity: &mut Validity) {
    let kind = forms::input_type(doc, input);
    let required = has(doc, input, "required");
    match kind {
        "checkbox" => {
            validity.value_missing = required && !forms::is_checked(doc, input);
            return;
        }
        "radio" => {
            validity.value_missing = radio_group_missing(doc, input);
            return;
        }
        "file" => {
            validity.value_missing = required;
            return;
        }
        "submit" | "image" | "range" | "color" => return,
        _ => {}
    }
    let value = forms::control_value(doc, input);
    validity.value_missing = required && value.is_empty();
    if value.is_empty() {
        return;
    }
    if text_like(kind) {
        validity.type_mismatch = match kind {
            "email" => !emails_valid(&value, has(doc, input, "multiple")),
            "url" => url::Url::parse(&value).is_err(),
            _ => false,
        };
        validity.pattern_mismatch = pattern_mismatch(doc, input, kind, &value);
        validity.too_short = too_short(doc, input, &value);
        return;
    }
    let Some(numeric) = Numeric::of(kind) else {
        return;
    };
    let Some(number) = numeric.parse(&value) else {
        validity.bad_input = true;
        return;
    };
    let min = doc
        .attribute_str(input, "min")
        .and_then(|min| numeric.parse(min));
    let max = doc
        .attribute_str(input, "max")
        .and_then(|max| numeric.parse(max));
    match (min, max) {
        // A time range may wrap past midnight: `min` after `max` allows the
        // values outside the gap between them.
        (Some(min), Some(max)) if kind == "time" && min > max => {
            let outside = number < min && number > max;
            validity.range_underflow = outside;
            validity.range_overflow = outside;
        }
        _ => {
            validity.range_underflow = min.is_some_and(|min| number < min);
            validity.range_overflow = max.is_some_and(|max| number > max);
        }
    }
    if let Some(step) = allowed_step(doc, input, numeric) {
        let base = step_base(doc, input, numeric);
        validity.step_mismatch = !on_step(number, base, step);
    }
}

/// Whether a radio button's group is required and has nothing checked. The
/// group is the radio buttons of the same tree and form owner with the same
/// non-empty name; a button without a name is a group of its own.
fn radio_group_missing(doc: &Document, radio: NodeId) -> bool {
    let name = doc.attribute_str(radio, "name").unwrap_or("");
    if name.is_empty() {
        return has(doc, radio, "required") && !forms::is_checked(doc, radio);
    }
    let owner = forms::form_owner(doc, radio);
    let root = doc.root_of(radio);
    let mut required = false;
    for member in doc.descendants(root).filter(|node| {
        doc.tag(*node) == Some(atoms::INPUT)
            && forms::input_type(doc, *node) == "radio"
            && doc.attribute_str(*node, "name") == Some(name)
            && forms::form_owner(doc, *node) == owner
    }) {
        if forms::is_checked(doc, member) {
            return false;
        }
        required |= has(doc, member, "required");
    }
    required
}

/// A valid email address, or with `multiple` a comma-separated list of
/// them (the standard's grammar, which is deliberately narrower than RFC
/// 5322).
fn emails_valid(value: &str, multiple: bool) -> bool {
    if multiple {
        value.split(',').all(|address| email_valid(address.trim()))
    } else {
        email_valid(value)
    }
}

fn email_valid(address: &str) -> bool {
    let Some((local, domain)) = address.split_once('@') else {
        return false;
    };
    const LOCAL_EXTRA: &str = ".!#$%&'*+/=?^_`{|}~-";
    let local_ok = !local.is_empty()
        && local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || LOCAL_EXTRA.contains(c));
    let domain_ok = !domain.is_empty()
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    local_ok && domain_ok
}

/// Whether the value fails the `pattern` attribute, matched against the
/// whole value (each address of a multiple email control).
fn pattern_mismatch(doc: &Document, input: NodeId, kind: &str, value: &str) -> bool {
    let Some(pattern) = doc.attribute_str(input, "pattern") else {
        return false;
    };
    let Ok(regex) = regex_lite::Regex::new(&format!("^(?:{pattern})$")) else {
        return false;
    };
    if kind == "email" && has(doc, input, "multiple") {
        value
            .split(',')
            .any(|address| !regex.is_match(address.trim()))
    } else {
        !regex.is_match(value)
    }
}

// ---- numbers and dates ----------------------------------------------------

/// The input types whose value is a number on a line: how to parse it, the
/// step's default and scale, and the default step base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Numeric {
    Number,
    Date,
    Month,
    Week,
    Time,
    DateTimeLocal,
}

const DAY_MS: f64 = 86_400_000.0;

impl Numeric {
    fn of(kind: &str) -> Option<Self> {
        Some(match kind {
            "number" => Self::Number,
            "date" => Self::Date,
            "month" => Self::Month,
            "week" => Self::Week,
            "time" => Self::Time,
            "datetime-local" => Self::DateTimeLocal,
            _ => return None,
        })
    }

    fn parse(self, text: &str) -> Option<f64> {
        match self {
            Self::Number => parse_float(text),
            Self::Date => parse_date(text).map(|days| days as f64 * DAY_MS),
            Self::Month => parse_month(text)
                .map(|(year, month)| ((year - 1970) * 12 + i64::from(month) - 1) as f64),
            Self::Week => parse_week(text),
            Self::Time => parse_time(text),
            Self::DateTimeLocal => {
                let (date, time) = text.split_once(['T', ' '])?;
                Some(parse_date(date)? as f64 * DAY_MS + parse_time(time)?)
            }
        }
    }

    fn default_step(self) -> f64 {
        match self {
            Self::Time | Self::DateTimeLocal => 60.0,
            _ => 1.0,
        }
    }

    fn step_scale(self) -> f64 {
        match self {
            Self::Number | Self::Month => 1.0,
            Self::Date => DAY_MS,
            Self::Week => 7.0 * DAY_MS,
            Self::Time | Self::DateTimeLocal => 1000.0,
        }
    }

    fn default_step_base(self) -> f64 {
        match self {
            // The Monday 1970-W01 begins on, 1969-12-29.
            Self::Week => -3.0 * DAY_MS,
            _ => 0.0,
        }
    }
}

/// HTML's rules for parsing a valid floating-point number, which are
/// stricter than Rust's: no leading `+`, no trailing `.`, no `inf`.
pub fn parse_float(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    let mut at = usize::from(bytes.first() == Some(&b'-'));
    let digits = |at: &mut usize| {
        let start = *at;
        while bytes.get(*at).is_some_and(u8::is_ascii_digit) {
            *at += 1;
        }
        *at > start
    };
    let whole = digits(&mut at);
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        if !digits(&mut at) {
            return None;
        }
    } else if !whole {
        return None;
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        at += 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        if !digits(&mut at) {
            return None;
        }
    }
    (at == bytes.len())
        .then(|| text.parse::<f64>().ok())
        .flatten()
        .filter(|number| number.is_finite())
}

fn number_of(text: &str, len: usize) -> Option<u32> {
    (text.len() == len && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to the date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// `YYYY-MM`: four or more digits of a year above 0, and a month.
fn parse_month(text: &str) -> Option<(i64, u32)> {
    let (year, month) = text.rsplit_once('-')?;
    if year.len() < 4 || !year.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i64 = year.parse().ok()?;
    let month = number_of(month, 2)?;
    (year > 0 && (1..=12).contains(&month)).then_some((year, month))
}

/// `YYYY-MM-DD` as days since 1970-01-01.
fn parse_date(text: &str) -> Option<i64> {
    let (month, day) = text.rsplit_once('-')?;
    let (year, month) = parse_month(month)?;
    let day = number_of(day, 2)?;
    (1..=days_in_month(year, month).max(1))
        .contains(&day)
        .then(|| days_from_civil(year, month, day))
}

/// `YYYY-Www` as the milliseconds of the week's Monday.
fn parse_week(text: &str) -> Option<f64> {
    let (year, week) = text.split_once("-W")?;
    if year.len() < 4 || !year.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i64 = year.parse().ok()?;
    let week = number_of(week, 2)?;
    // The weekday of 1 January, Monday 0, decides whether the year has 53
    // weeks: it does when it starts on a Thursday, or on a Wednesday in a
    // leap year.
    let jan1 = days_from_civil(year, 1, 1);
    let weekday = (jan1 + 3).rem_euclid(7);
    let leap = days_in_month(year, 2) == 29;
    let weeks = if weekday == 3 || (weekday == 2 && leap) {
        53
    } else {
        52
    };
    if year <= 0 || !(1..=weeks).contains(&week) {
        return None;
    }
    // Week 1 holds the year's first Thursday; its Monday is at most three
    // days before 1 January.
    let first_monday = if weekday <= 3 {
        jan1 - weekday
    } else {
        jan1 + 7 - weekday
    };
    Some((first_monday + 7 * (i64::from(week) - 1)) as f64 * DAY_MS)
}

/// `HH:MM`, `HH:MM:SS` or `HH:MM:SS.sss` as milliseconds after midnight.
fn parse_time(text: &str) -> Option<f64> {
    let mut parts = text.splitn(3, ':');
    let hours = number_of(parts.next()?, 2)?;
    let minutes = number_of(parts.next()?, 2)?;
    let seconds = match parts.next() {
        None => 0.0,
        Some(seconds) => {
            let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
            let whole = number_of(whole, 2)?;
            if whole > 59
                || (seconds.contains('.')
                    && (fraction.is_empty()
                        || fraction.len() > 3
                        || !fraction.bytes().all(|b| b.is_ascii_digit())))
            {
                return None;
            }
            let fraction = if fraction.is_empty() {
                0.0
            } else {
                format!("0.{fraction}").parse::<f64>().ok()?
            };
            f64::from(whole) + fraction
        }
    };
    (hours <= 23 && minutes <= 59)
        .then(|| (f64::from(hours) * 3600.0 + f64::from(minutes) * 60.0 + seconds) * 1000.0)
}

/// The allowed value step, scaled to the value's unit; `None` for
/// `step="any"`.
fn allowed_step(doc: &Document, input: NodeId, numeric: Numeric) -> Option<f64> {
    let given = doc.attribute_str(input, "step").map(str::trim);
    if given.is_some_and(|step| step.eq_ignore_ascii_case("any")) {
        return None;
    }
    let step = given
        .and_then(parse_float)
        .filter(|step| *step > 0.0)
        .map(|step| match numeric {
            // A month or a day cannot be split: the step rounds to whole ones.
            Numeric::Month | Numeric::Date | Numeric::Week => step.round().max(1.0),
            _ => step,
        })
        .unwrap_or_else(|| numeric.default_step());
    Some(step * numeric.step_scale())
}

fn step_base(doc: &Document, input: NodeId, numeric: Numeric) -> f64 {
    ["min", "value"]
        .iter()
        .find_map(|name| {
            doc.attribute_str(input, name)
                .and_then(|text| numeric.parse(text))
        })
        .unwrap_or_else(|| numeric.default_step_base())
}

/// Whether `value` is a whole number of steps from `base`, allowing the
/// error that decimal steps such as 0.1 pick up in binary.
fn on_step(value: f64, base: f64, step: f64) -> bool {
    let steps = (value - base) / step;
    (steps - steps.round()).abs() <= 1e-7 * steps.abs().max(1.0)
}

// ---- the value as a number -------------------------------------------------

/// Civil date of a day count since 1970-01-01 (Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn format_date(days: i64) -> String {
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// A time of day in its shortest valid form: seconds and milliseconds only
/// when they are not zero.
fn format_time(ms: f64) -> String {
    let ms = ms.rem_euclid(DAY_MS).round() as i64;
    let (hours, minutes) = (ms / 3_600_000, ms / 60_000 % 60);
    let (seconds, millis) = (ms / 1000 % 60, ms % 1000);
    let mut out = format!("{hours:02}:{minutes:02}");
    if seconds != 0 || millis != 0 {
        out.push_str(&format!(":{seconds:02}"));
        if millis != 0 {
            out.push_str(&format!(".{millis:03}"));
        }
    }
    out
}

impl Numeric {
    /// The value string a number stands for, as the standard serializes it.
    fn format(self, number: f64) -> Option<String> {
        if !number.is_finite() {
            return None;
        }
        Some(match self {
            Self::Number => shortest(number),
            Self::Date => format_date((number / DAY_MS).floor() as i64),
            Self::Month => {
                let months = number.floor() as i64;
                format!(
                    "{:04}-{:02}",
                    1970 + months.div_euclid(12),
                    months.rem_euclid(12) + 1
                )
            }
            Self::Week => {
                let days = (number / DAY_MS).floor() as i64;
                // The week's Thursday names its year.
                let thursday = days - (days + 3).rem_euclid(7) + 3;
                let (year, _, _) = civil_from_days(thursday);
                let week = (thursday - days_from_civil(year, 1, 1)) / 7 + 1;
                format!("{year:04}-W{week:02}")
            }
            Self::Time => format_time(number),
            Self::DateTimeLocal => {
                let days = (number / DAY_MS).floor() as i64;
                format!(
                    "{}T{}",
                    format_date(days),
                    format_time(number - days as f64 * DAY_MS)
                )
            }
        })
    }
}

/// Whether the input's type has a value as a number (`valueAsNumber`).
pub fn has_numeric_value(doc: &Document, input: NodeId) -> bool {
    Numeric::of(forms::input_type(doc, input)).is_some()
}

/// `valueAsNumber`: the value as a number (a date as milliseconds since
/// the epoch, a month as months since 1970), NaN when it has none.
pub fn value_as_number(doc: &Document, input: NodeId) -> f64 {
    Numeric::of(forms::input_type(doc, input))
        .and_then(|numeric| numeric.parse(&forms::control_value(doc, input)))
        .unwrap_or(f64::NAN)
}

/// The value string `valueAsNumber = number` writes: empty for NaN,
/// `None` when the type has no value as a number.
pub fn number_to_value(doc: &Document, input: NodeId, number: f64) -> Option<String> {
    let numeric = Numeric::of(forms::input_type(doc, input))?;
    Some(if number.is_nan() {
        String::new()
    } else {
        numeric.format(number).unwrap_or_default()
    })
}

/// Why `stepUp()` or `stepDown()` cannot step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepError {
    /// The type has no value as a number, or the step is `any`.
    NotApplicable,
}

/// `stepUp(n)` (or `stepDown` with a negative `n`): the value string the
/// control steps to, `Ok(None)` when it stays as it is.
pub fn stepped_value(doc: &Document, input: NodeId, n: i32) -> Result<Option<String>, StepError> {
    let numeric = Numeric::of(forms::input_type(doc, input)).ok_or(StepError::NotApplicable)?;
    let step = allowed_step(doc, input, numeric).ok_or(StepError::NotApplicable)?;
    let min = doc
        .attribute_str(input, "min")
        .and_then(|min| numeric.parse(min));
    let max = doc
        .attribute_str(input, "max")
        .and_then(|max| numeric.parse(max));
    if let (Some(min), Some(max)) = (min, max)
        && min > max
    {
        return Ok(None);
    }
    let base = step_base(doc, input, numeric);
    let before = numeric
        .parse(&forms::control_value(doc, input))
        .unwrap_or(0.0);
    let mut value = if on_step(before, base, step) {
        before + step * f64::from(n)
    } else if n < 0 {
        base + ((before - base) / step).floor() * step
    } else {
        base + ((before - base) / step).ceil() * step
    };
    if let Some(min) = min
        && value < min
    {
        value = base + ((min - base) / step).ceil() * step;
    }
    if let Some(max) = max
        && value > max
    {
        value = base + ((max - base) / step).floor() * step;
    }
    if (n < 0 && value > before) || (n > 0 && value < before) {
        return Ok(None);
    }
    Ok(numeric.format(value))
}

// ---- messages -------------------------------------------------------------

/// `validationMessage`: what a browser would show for the first failure,
/// in Chrome's English wording; empty when the control is valid or is not
/// a candidate.
pub fn validation_message(doc: &Document, control: NodeId) -> String {
    if !will_validate(doc, control) {
        return String::new();
    }
    let validity = validity(doc, control);
    if let Some(custom) = doc.custom_validity(control) {
        return custom.to_owned();
    }
    let kind = match doc.tag(control) {
        Some(tag) if tag == atoms::INPUT => forms::input_type(doc, control),
        Some(tag) if tag == atoms::SELECT => "select",
        _ => "textarea",
    };
    let value = forms::control_value(doc, control);
    let attribute = |name: &str| doc.attribute_str(control, name).unwrap_or("").to_owned();
    let dated = matches!(kind, "date" | "month" | "week" | "time" | "datetime-local");
    if validity.value_missing {
        return match kind {
            "checkbox" => "Please check this box if you want to proceed.",
            "radio" => "Please select one of these options.",
            "select" => "Please select an item in the list.",
            "file" => "Please select a file.",
            _ => "Please fill out this field.",
        }
        .to_owned();
    }
    if validity.type_mismatch {
        return if kind == "url" {
            "Please enter a URL.".to_owned()
        } else if !value.contains('@') {
            format!("Please include an '@' in the email address. '{value}' is missing an '@'.")
        } else {
            "Please enter an email address.".to_owned()
        };
    }
    if validity.bad_input {
        return if kind == "number" {
            "Please enter a number."
        } else {
            "Please enter a valid value."
        }
        .to_owned();
    }
    if validity.pattern_mismatch {
        return "Please match the requested format.".to_owned();
    }
    if validity.too_short {
        let length = value.encode_utf16().count();
        let unit = if length == 1 {
            "character"
        } else {
            "characters"
        };
        return format!(
            "Please lengthen this text to {} characters or more (you are currently using {length} {unit}).",
            attribute("minlength").trim()
        );
    }
    if validity.range_underflow {
        return if dated {
            format!("Value must be {} or later.", attribute("min"))
        } else {
            format!(
                "Value must be greater than or equal to {}.",
                attribute("min")
            )
        };
    }
    if validity.range_overflow {
        return if dated {
            format!("Value must be {} or earlier.", attribute("max"))
        } else {
            format!("Value must be less than or equal to {}.", attribute("max"))
        };
    }
    if validity.step_mismatch {
        if kind == "number"
            && let (Some(number), Some(step)) = (
                parse_float(&value),
                allowed_step(doc, control, Numeric::Number),
            )
        {
            let base = step_base(doc, control, Numeric::Number);
            let below = base + ((number - base) / step).floor() * step;
            let above = below + step;
            return format!(
                "Please enter a valid value. The two nearest valid values are {} and {}.",
                shortest(below),
                shortest(above)
            );
        }
        return "Please enter a valid value.".to_owned();
    }
    String::new()
}

/// A number as script would print it, without the binary noise a step
/// computation leaves (`0.30000000000000004` is `0.3`).
fn shortest(number: f64) -> String {
    let rounded = (number * 1e9).round() / 1e9;
    let text = format!("{rounded}");
    if text == "-0" { "0".to_owned() } else { text }
}

// ---- forms ----------------------------------------------------------------

/// The controls of `form` that are candidates for constraint validation and
/// fail it, in tree order: the form's "statically validate the
/// constraints".
pub fn invalid_controls(doc: &Document, form: NodeId) -> Vec<NodeId> {
    forms::form_controls(doc, form)
        .into_iter()
        .filter(|control| will_validate(doc, *control) && !validity(doc, *control).valid())
        .collect()
}

/// The no-validate state of a submission: the submitter's `formnovalidate`,
/// or the form's `novalidate`.
pub fn skips_validation(doc: &Document, form: NodeId, submitter: Option<NodeId>) -> bool {
    submitter.is_some_and(|submitter| has(doc, submitter, "formnovalidate"))
        || has(doc, form, "novalidate")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_follow_html_rules() {
        assert_eq!(parse_float("1.5"), Some(1.5));
        assert_eq!(parse_float("-.5"), Some(-0.5));
        assert_eq!(parse_float("1e3"), Some(1000.0));
        for bad in ["", "+1", "1.", ".", "inf", "1e", " 1", "0x10", "-"] {
            assert_eq!(parse_float(bad), None, "{bad}");
        }
    }

    #[test]
    fn dates_and_times_parse_to_their_units() {
        assert_eq!(parse_date("1970-01-02"), Some(1));
        assert_eq!(parse_date("2024-02-29"), Some(19_782));
        assert_eq!(parse_date("2023-02-29"), None);
        assert_eq!(parse_month("1970-02"), Some((1970, 2)));
        assert_eq!(parse_time("01:02"), Some(3_720_000.0));
        assert_eq!(parse_time("00:00:01.5"), Some(1500.0));
        assert_eq!(parse_time("24:00"), None);
        // 2020-W01 begins on Monday 2019-12-30; 2020 has 53 weeks.
        assert_eq!(
            parse_week("2020-W01"),
            Some(days_from_civil(2019, 12, 30) as f64 * DAY_MS)
        );
        assert!(parse_week("2020-W53").is_some());
        assert!(parse_week("2021-W53").is_none());
        assert_eq!(parse_week("1970-W01"), Some(-3.0 * DAY_MS));
    }

    #[test]
    fn numbers_format_back_to_value_strings() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(
            Numeric::Date.format(19_782.0 * DAY_MS).unwrap(),
            "2024-02-29"
        );
        assert_eq!(Numeric::Month.format(-1.0).unwrap(), "1969-12");
        let week = parse_week("2020-W53").unwrap();
        assert_eq!(Numeric::Week.format(week).unwrap(), "2020-W53");
        assert_eq!(Numeric::Week.format(-3.0 * DAY_MS).unwrap(), "1970-W01");
        assert_eq!(Numeric::Time.format(3_723_400.0).unwrap(), "01:02:03.400");
        assert_eq!(Numeric::Time.format(60_000.0).unwrap(), "00:01");
        assert_eq!(
            Numeric::DateTimeLocal.format(DAY_MS + 90_000.0).unwrap(),
            "1970-01-02T00:01:30"
        );
        assert_eq!(Numeric::Number.format(0.1 + 0.2).unwrap(), "0.3");
    }

    #[test]
    fn steps_tolerate_decimal_error() {
        assert!(on_step(0.3, 0.0, 0.1));
        assert!(!on_step(0.35, 0.0, 0.1));
        assert!(on_step(7.0, 1.0, 2.0));
        assert!(!on_step(6.0, 1.0, 2.0));
    }

    #[test]
    fn email_addresses_follow_the_standard_grammar() {
        assert!(email_valid("a.b+c@example.com"));
        assert!(email_valid("a@localhost"));
        for bad in ["", "a", "a@", "@b", "a@-b.com", "a@b..com", "a b@c"] {
            assert!(!email_valid(bad), "{bad}");
        }
        assert!(emails_valid("a@b, c@d", true));
        assert!(!emails_valid("a@b, c", true));
    }
}
