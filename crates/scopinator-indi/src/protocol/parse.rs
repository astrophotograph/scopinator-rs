//! Reading the stream an INDI server sends.
//!
//! An INDI connection carries one XML element after another with nothing
//! around them, and TCP hands them over in pieces of any size. The
//! [`StreamParser`] finds where each top-level element ends, and each
//! complete element is then read as one [`IndiMessage`].

use base64::Engine;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::error::IndiError;
use crate::protocol::message::{
    Blob, IndiMessage, MemberDefinition, MemberValue, NumberLimits, Permission, PropertyDefinition,
    PropertyUpdate, SwitchRule, VectorKind,
};
use crate::protocol::number::parse_number;
use crate::protocol::property::{PropertyState, SwitchState};

/// The most one element may take before it is given up on: enough for a
/// large camera frame in base64.
pub const DEFAULT_MAX_ELEMENT: usize = 64 * 1024 * 1024;

/// The most one tag (a name and its attributes) may take.
const MAX_TAG: usize = 1024 * 1024;

/// How much of the end of the buffer is kept while looking for the next
/// element after losing the place: the start of one may be split there.
const RESYNC_TAIL: usize = 16;

/// The elements a server sends, by how they begin. Used to find the stream
/// again after an element that had to be thrown away.
const ELEMENT_STARTS: [&[u8]; 4] = [b"<def", b"<set", b"<delProperty", b"<message"];

/// Splits a byte stream into INDI messages.
#[derive(Debug)]
pub struct StreamParser {
    /// Bytes not yet turned into messages. Starts at the element in
    /// progress, if there is one.
    buf: Vec<u8>,
    /// How far into `buf` has been looked at.
    scan: usize,
    /// How many elements are open at `scan`.
    depth: usize,
    max_element: usize,
    /// The place in the stream was lost: skip to something that looks like
    /// the start of an element.
    resync: bool,
    /// Text outside any element has been reported, and no element has
    /// started since.
    garbage: bool,
}

impl Default for StreamParser {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tag {
    Open,
    Close,
    /// `<name/>`.
    Empty,
    /// A declaration, a comment or a CDATA section.
    Other,
}

impl StreamParser {
    pub fn new() -> Self {
        Self::with_max_element(DEFAULT_MAX_ELEMENT)
    }

    /// A parser that gives up on an element larger than `max_element`
    /// bytes.
    pub fn with_max_element(max_element: usize) -> Self {
        Self {
            buf: Vec::new(),
            scan: 0,
            depth: 0,
            max_element,
            resync: false,
            garbage: false,
        }
    }

    /// Add bytes from the socket and return every message they complete,
    /// in order. An element that cannot be read is one `Err`; the elements
    /// after it are read as usual.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Result<IndiMessage, IndiError>> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        // Where the element in progress starts; everything before it is done
        // with.
        let mut base = 0;

        loop {
            if self.resync {
                match plausible_start(&self.buf[self.scan..]) {
                    Some(offset) => {
                        self.scan += offset;
                        base = self.scan;
                        self.resync = false;
                        self.depth = 0;
                    }
                    None => {
                        let keep = self.buf.len().saturating_sub(RESYNC_TAIL).max(self.scan);
                        self.scan = keep;
                        base = keep;
                        break;
                    }
                }
            }

            let rest = &self.buf[self.scan..];
            let Some(offset) = rest.iter().position(|&b| b == b'<') else {
                if self.depth == 0 {
                    self.note_text(rest.to_vec().as_slice(), &mut out);
                    base = self.buf.len();
                }
                self.scan = self.buf.len();
                break;
            };
            let at = self.scan + offset;
            if self.depth == 0 {
                let between = self.buf[self.scan..at].to_vec();
                self.note_text(&between, &mut out);
                self.garbage = false;
                base = at;
            }

            let Some((len, tag)) = tag_end(&self.buf[at..]) else {
                // The tag is not all here yet.
                self.scan = at;
                if self.buf.len() - at > MAX_TAG {
                    out.push(Err(IndiError::Malformed(format!(
                        "a tag longer than {MAX_TAG} bytes"
                    ))));
                    return self.lose_place(out);
                }
                break;
            };
            self.scan = at + len;
            match tag {
                Tag::Open => self.depth += 1,
                Tag::Close if self.depth == 0 => {
                    out.push(Err(IndiError::Malformed(
                        "a closing tag with nothing open".into(),
                    )));
                    base = self.scan;
                    continue;
                }
                Tag::Close => self.depth -= 1,
                Tag::Empty | Tag::Other => {}
            }
            if self.depth == 0 {
                if matches!(tag, Tag::Close | Tag::Empty) {
                    out.push(parse_element(&self.buf[base..self.scan]));
                }
                base = self.scan;
            }
        }

        if self.buf.len() - base > self.max_element {
            out.push(Err(IndiError::TooLarge {
                limit: self.max_element,
            }));
            return self.lose_place(out);
        }
        self.buf.drain(..base);
        self.scan -= base;
        out
    }

    /// Report text found outside any element, once for each run of it.
    fn note_text(&mut self, text: &[u8], out: &mut Vec<Result<IndiMessage, IndiError>>) {
        if !self.garbage && text.iter().any(|b| !b.is_ascii_whitespace()) {
            out.push(Err(IndiError::Malformed("text outside any element".into())));
            self.garbage = true;
        }
    }

    /// Throw the buffer away and look for the next element from here on.
    fn lose_place(
        &mut self,
        out: Vec<Result<IndiMessage, IndiError>>,
    ) -> Vec<Result<IndiMessage, IndiError>> {
        self.buf.clear();
        self.scan = 0;
        self.depth = 0;
        self.resync = true;
        self.garbage = false;
        out
    }
}

/// Where in `buf` something that looks like the start of an element a
/// server sends begins. `None` if there is none, or not enough of the last
/// candidate to tell.
fn plausible_start(buf: &[u8]) -> Option<usize> {
    let longest = ELEMENT_STARTS.iter().map(|s| s.len()).max().unwrap_or(0);
    let mut from = 0;
    while let Some(offset) = buf[from..].iter().position(|&b| b == b'<') {
        let at = from + offset;
        let here = &buf[at..];
        if ELEMENT_STARTS.iter().any(|start| here.starts_with(start)) {
            return Some(at);
        }
        if here.len() < longest {
            // It may be one once the rest of it arrives.
            return None;
        }
        from = at + 1;
    }
    None
}

/// The length and kind of the tag `buf` begins with (its first byte is
/// `<`), or `None` if its end has not arrived.
fn tag_end(buf: &[u8]) -> Option<(usize, Tag)> {
    let find = |needle: &[u8]| {
        buf.windows(needle.len())
            .position(|window| window == needle)
            .map(|at| (at + needle.len(), Tag::Other))
    };
    if buf.starts_with(b"<?") {
        return find(b"?>");
    }
    // Not enough yet to tell a comment or a CDATA section from another
    // declaration.
    for opening in [&b"<!--"[..], &b"<![CDATA["[..]] {
        if buf.len() < opening.len() && opening.starts_with(buf) {
            return None;
        }
    }
    if buf.starts_with(b"<!--") {
        return find(b"-->");
    }
    if buf.starts_with(b"<![CDATA[") {
        return find(b"]]>");
    }
    if buf.starts_with(b"<!") {
        return find(b">");
    }

    let closing = buf.starts_with(b"</");
    let mut quote = None;
    for (index, &byte) in buf.iter().enumerate().skip(1) {
        match quote {
            Some(open) if byte == open => quote = None,
            Some(_) => {}
            None => match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'>' => {
                    let tag = if closing {
                        Tag::Close
                    } else if buf[index - 1] == b'/' {
                        Tag::Empty
                    } else {
                        Tag::Open
                    };
                    return Some((index + 1, tag));
                }
                _ => {}
            },
        }
    }
    None
}

fn malformed(error: impl std::fmt::Display) -> IndiError {
    IndiError::Malformed(error.to_string())
}

/// An attribute's value, unescaped, or `None` if the tag does not have it.
fn attribute(tag: &BytesStart<'_>, name: &str) -> Result<Option<String>, IndiError> {
    match tag.try_get_attribute(name).map_err(malformed)? {
        Some(attribute) => Ok(Some(
            attribute.unescape_value().map_err(malformed)?.into_owned(),
        )),
        None => Ok(None),
    }
}

fn required(tag: &BytesStart<'_>, name: &str) -> Result<String, IndiError> {
    attribute(tag, name)?.ok_or_else(|| {
        malformed(format!(
            "<{}> without `{name}`",
            String::from_utf8_lossy(tag.name().as_ref())
        ))
    })
}

/// An attribute that is absent or empty is not there.
fn optional(tag: &BytesStart<'_>, name: &str) -> Result<Option<String>, IndiError> {
    Ok(attribute(tag, name)?.filter(|value| !value.is_empty()))
}

fn number_attribute(tag: &BytesStart<'_>, name: &str) -> Result<Option<f64>, IndiError> {
    match optional(tag, name)? {
        Some(text) => parse_number(&text)
            .map(Some)
            .ok_or_else(|| malformed(format!("`{name}` is {text:?}, not a number"))),
        None => Ok(None),
    }
}

/// One member element: its tag and the text inside it.
struct Member {
    tag: BytesStart<'static>,
    text: String,
}

/// Read the member elements of a vector up to its end.
fn members(reader: &mut Reader<&[u8]>, wanted: &str) -> Result<Vec<Member>, IndiError> {
    let mut found = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Start(tag) if tag.name().as_ref() == wanted.as_bytes() => {
                let tag = tag.into_owned();
                let mut text = String::new();
                loop {
                    match reader.read_event()? {
                        Event::Text(part) => text.push_str(&part.unescape().map_err(malformed)?),
                        Event::CData(part) => text.push_str(&String::from_utf8_lossy(&part)),
                        Event::End(_) => break,
                        Event::Eof => return Err(malformed(format!("<{wanted}> does not end"))),
                        _ => {}
                    }
                }
                found.push(Member { tag, text });
            }
            Event::Empty(tag) if tag.name().as_ref() == wanted.as_bytes() => {
                found.push(Member {
                    tag: tag.into_owned(),
                    text: String::new(),
                });
            }
            // A member of a kind this library does not know.
            Event::Start(tag) => {
                reader.read_to_end(tag.to_owned().name())?;
            }
            Event::End(_) | Event::Eof => return Ok(found),
            _ => {}
        }
    }
}

fn value(kind: VectorKind, member: &Member, defining: bool) -> Result<MemberValue, IndiError> {
    let name = attribute(&member.tag, "name")?.unwrap_or_default();
    let text = member.text.trim();
    let wrong = |what: &str| malformed(format!("{what} `{name}` is {text:?}"));
    Ok(match kind {
        VectorKind::Number => {
            MemberValue::Number(parse_number(text).ok_or_else(|| wrong("number"))?)
        }
        VectorKind::Switch => {
            MemberValue::Switch(SwitchState::try_parse(text).ok_or_else(|| wrong("switch"))?)
        }
        VectorKind::Text => MemberValue::Text(text.to_string()),
        VectorKind::Light => {
            MemberValue::Light(PropertyState::try_parse(text).ok_or_else(|| wrong("light"))?)
        }
        VectorKind::Blob if defining => MemberValue::NoBlob,
        VectorKind::Blob => {
            let format = attribute(&member.tag, "format")?.unwrap_or_default();
            let encoded: Vec<u8> = text
                .bytes()
                .filter(|byte| !byte.is_ascii_whitespace())
                .collect();
            let data = base64::engine::general_purpose::STANDARD
                .decode(&encoded)
                .map_err(|e| IndiError::Blob(format!("`{name}` is not base64: {e}")))?;
            // `size` is the size of the data once uncompressed.
            if !format.ends_with(".z")
                && let Some(size) = optional(&member.tag, "size")?
                && size.trim().parse::<usize>().ok() != Some(data.len())
            {
                return Err(IndiError::Blob(format!(
                    "`{name}` says it has {size} bytes and has {}",
                    data.len()
                )));
            }
            MemberValue::Blob(Blob { format, data })
        }
    })
}

fn state(tag: &BytesStart<'_>) -> Result<Option<PropertyState>, IndiError> {
    match optional(tag, "state")? {
        Some(text) => PropertyState::try_parse(&text)
            .map(Some)
            .ok_or_else(|| malformed(format!("state {text:?}"))),
        None => Ok(None),
    }
}

fn define(
    reader: &mut Reader<&[u8]>,
    root: &BytesStart<'_>,
    kind: VectorKind,
    member: &str,
) -> Result<IndiMessage, IndiError> {
    let permission = match optional(root, "perm")? {
        Some(text) => {
            Permission::parse(&text).ok_or_else(|| malformed(format!("perm {text:?}")))?
        }
        // Lights are for reading and say nothing about it.
        None if kind == VectorKind::Light => Permission::ReadOnly,
        None => return Err(malformed("a property without `perm`")),
    };
    let rule = match optional(root, "rule")? {
        Some(text) => {
            Some(SwitchRule::parse(&text).ok_or_else(|| malformed(format!("rule {text:?}")))?)
        }
        None => None,
    };
    let mut defined = Vec::new();
    for member in members(reader, member)? {
        let limits = if kind == VectorKind::Number {
            Some(NumberLimits {
                format: attribute(&member.tag, "format")?.unwrap_or_else(|| "%g".into()),
                min: number_attribute(&member.tag, "min")?.unwrap_or(0.0),
                max: number_attribute(&member.tag, "max")?.unwrap_or(0.0),
                step: number_attribute(&member.tag, "step")?.unwrap_or(0.0),
            })
        } else {
            None
        };
        defined.push(MemberDefinition {
            name: required(&member.tag, "name")?,
            label: optional(&member.tag, "label")?,
            value: value(kind, &member, true)?,
            limits,
        });
    }
    Ok(IndiMessage::Define(PropertyDefinition {
        kind,
        device: required(root, "device")?,
        name: required(root, "name")?,
        label: optional(root, "label")?,
        group: optional(root, "group")?,
        state: state(root)?.ok_or_else(|| malformed("a property without `state`"))?,
        permission,
        rule,
        timeout: number_attribute(root, "timeout")?,
        timestamp: optional(root, "timestamp")?,
        message: optional(root, "message")?,
        members: defined,
    }))
}

fn update(
    reader: &mut Reader<&[u8]>,
    root: &BytesStart<'_>,
    kind: VectorKind,
    member: &str,
) -> Result<IndiMessage, IndiError> {
    let mut values = Vec::new();
    for member in members(reader, member)? {
        values.push((required(&member.tag, "name")?, value(kind, &member, false)?));
    }
    Ok(IndiMessage::Update(PropertyUpdate {
        kind,
        device: required(root, "device")?,
        name: required(root, "name")?,
        state: state(root)?,
        timeout: number_attribute(root, "timeout")?,
        timestamp: optional(root, "timestamp")?,
        message: optional(root, "message")?,
        members: values,
    }))
}

/// Read one complete top-level element.
pub fn parse_element(element: &[u8]) -> Result<IndiMessage, IndiError> {
    let mut reader = Reader::from_reader(element);
    let (root, empty) = loop {
        match reader.read_event()? {
            Event::Start(tag) => break (tag.into_owned(), false),
            Event::Empty(tag) => break (tag.into_owned(), true),
            Event::Eof => return Err(malformed("no element")),
            _ => {}
        }
    };
    let name = String::from_utf8_lossy(root.name().as_ref()).into_owned();
    // A vector written as an empty element has no members to read.
    let mut none = Reader::from_reader(&b""[..]);
    let reader = if empty { &mut none } else { &mut reader };
    use VectorKind::*;
    match name.as_str() {
        "defNumberVector" => define(reader, &root, Number, "defNumber"),
        "defSwitchVector" => define(reader, &root, Switch, "defSwitch"),
        "defTextVector" => define(reader, &root, Text, "defText"),
        "defLightVector" => define(reader, &root, Light, "defLight"),
        "defBLOBVector" => define(reader, &root, Blob, "defBLOB"),
        "setNumberVector" => update(reader, &root, Number, "oneNumber"),
        "setSwitchVector" => update(reader, &root, Switch, "oneSwitch"),
        "setTextVector" => update(reader, &root, Text, "oneText"),
        "setLightVector" => update(reader, &root, Light, "oneLight"),
        "setBLOBVector" => update(reader, &root, Blob, "oneBLOB"),
        "delProperty" => Ok(IndiMessage::Delete {
            device: required(&root, "device")?,
            name: optional(&root, "name")?,
        }),
        "message" => Ok(IndiMessage::Message {
            device: optional(&root, "device")?,
            timestamp: optional(&root, "timestamp")?,
            text: attribute(&root, "message")?.unwrap_or_default(),
        }),
        _ => Ok(IndiMessage::Unknown { element: name }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A telescope defining itself and then moving, as `indiserver` sends
    /// it.
    const SESSION: &str = r#"<defSwitchVector device="Telescope Simulator" name="CONNECTION" label="Connection" group="Main Control" state="Idle" perm="rw" rule="OneOfMany" timeout="60" timestamp="2026-11-27T04:00:00">
    <defSwitch name="CONNECT" label="Connect">
Off
    </defSwitch>
    <defSwitch name="DISCONNECT" label="Disconnect">
On
    </defSwitch>
</defSwitchVector>
<defNumberVector device="Telescope Simulator" name="EQUATORIAL_EOD_COORD" label="Eq. Coordinates" group="Main Control" state="Idle" perm="rw" timeout="60" timestamp="2026-11-27T04:00:01">
    <defNumber name="RA" label="RA (hh:mm:ss)" format="%010.6m" min="0" max="24" step="0">
 0:00:00
    </defNumber>
    <defNumber name="DEC" label="DEC (dd:mm:ss)" format="%010.6m" min="-90" max="90" step="0">
-00:30:00
    </defNumber>
</defNumberVector>
<defTextVector device="Telescope Simulator" name="DRIVER_INFO" label="Driver Info" group="General Info" state="Idle" perm="ro" timeout="60" timestamp="2026-11-27T04:00:01">
    <defText name="DRIVER_NAME" label="Name">Telescope Simulator</defText>
    <defText name="DRIVER_INTERFACE" label="Interface">5</defText>
</defTextVector>
<defLightVector device="Weather" name="WEATHER_STATUS" label="Status" group="Main" state="Ok">
    <defLight name="WEATHER_WIND" label="Wind">Ok</defLight>
</defLightVector>
<defBLOBVector device="CCD Simulator" name="CCD1" label="Image Data" group="Image Info" state="Idle" perm="ro" timeout="60">
    <defBLOB name="CCD1" label="Image"/>
</defBLOBVector>
<setNumberVector device="Telescope Simulator" name="EQUATORIAL_EOD_COORD" state="Busy" timeout="60" timestamp="2026-11-27T04:00:05">
    <oneNumber name="RA">12.5125</oneNumber>
    <oneNumber name="DEC">45:30</oneNumber>
</setNumberVector>
<message device="Telescope Simulator" timestamp="2026-11-27T04:00:06" message="Slewing to RA 12:30:45 &amp; DEC 45:30"/>
<delProperty device="Telescope Simulator" name="TELESCOPE_PARK"/>
<delProperty device="Weather"/>
"#;

    fn all(text: &str) -> Vec<IndiMessage> {
        StreamParser::new()
            .feed(text.as_bytes())
            .into_iter()
            .map(|message| message.expect("a message"))
            .collect()
    }

    #[test]
    fn a_session_is_read_message_by_message() {
        let messages = all(SESSION);
        assert_eq!(messages.len(), 9);

        let IndiMessage::Define(connection) = &messages[0] else {
            panic!("{:?}", messages[0]);
        };
        assert_eq!(connection.kind, VectorKind::Switch);
        assert_eq!(connection.device, "Telescope Simulator");
        assert_eq!(connection.name, "CONNECTION");
        assert_eq!(connection.group.as_deref(), Some("Main Control"));
        assert_eq!(connection.state, PropertyState::Idle);
        assert_eq!(connection.permission, Permission::ReadWrite);
        assert_eq!(connection.rule, Some(SwitchRule::OneOfMany));
        assert_eq!(connection.timeout, Some(60.0));
        assert_eq!(connection.members.len(), 2);
        assert_eq!(connection.members[0].name, "CONNECT");
        assert_eq!(connection.members[0].label.as_deref(), Some("Connect"));
        assert_eq!(
            connection.members[0].value,
            MemberValue::Switch(SwitchState::Off)
        );
        assert_eq!(
            connection.members[1].value,
            MemberValue::Switch(SwitchState::On)
        );

        let IndiMessage::Define(coordinates) = &messages[1] else {
            panic!("{:?}", messages[1]);
        };
        assert_eq!(coordinates.kind, VectorKind::Number);
        // Sexagesimal, with the sign on the whole value.
        assert_eq!(coordinates.members[0].value, MemberValue::Number(0.0));
        assert_eq!(coordinates.members[1].value, MemberValue::Number(-0.5));
        let limits = coordinates.members[1].limits.as_ref().unwrap();
        assert_eq!((limits.min, limits.max, limits.step), (-90.0, 90.0, 0.0));
        assert_eq!(limits.format, "%010.6m");

        let IndiMessage::Define(driver) = &messages[2] else {
            panic!("{:?}", messages[2]);
        };
        assert_eq!(driver.permission, Permission::ReadOnly);
        assert_eq!(driver.members[1].value, MemberValue::Text("5".into()));

        let IndiMessage::Define(weather) = &messages[3] else {
            panic!("{:?}", messages[3]);
        };
        // A light vector has no `perm`: it is for reading.
        assert_eq!(weather.kind, VectorKind::Light);
        assert_eq!(weather.permission, Permission::ReadOnly);
        assert_eq!(
            weather.members[0].value,
            MemberValue::Light(PropertyState::Ok)
        );

        let IndiMessage::Define(image) = &messages[4] else {
            panic!("{:?}", messages[4]);
        };
        assert_eq!(image.kind, VectorKind::Blob);
        assert_eq!(image.members[0].value, MemberValue::NoBlob);

        let IndiMessage::Update(moving) = &messages[5] else {
            panic!("{:?}", messages[5]);
        };
        assert_eq!(moving.state, Some(PropertyState::Busy));
        assert_eq!(
            moving.members,
            [
                ("RA".to_string(), MemberValue::Number(12.5125)),
                ("DEC".to_string(), MemberValue::Number(45.5)),
            ]
        );

        assert_eq!(
            messages[6],
            IndiMessage::Message {
                device: Some("Telescope Simulator".into()),
                timestamp: Some("2026-11-27T04:00:06".into()),
                // Unescaped.
                text: "Slewing to RA 12:30:45 & DEC 45:30".into(),
            }
        );
        assert_eq!(
            messages[7],
            IndiMessage::Delete {
                device: "Telescope Simulator".into(),
                name: Some("TELESCOPE_PARK".into()),
            }
        );
        assert_eq!(
            messages[8],
            IndiMessage::Delete {
                device: "Weather".into(),
                name: None,
            }
        );
    }

    #[test]
    fn one_byte_at_a_time_gives_the_same_messages() {
        let whole = all(SESSION);
        let mut parser = StreamParser::new();
        let mut piecewise = Vec::new();
        for byte in SESSION.as_bytes() {
            for message in parser.feed(std::slice::from_ref(byte)) {
                piecewise.push(message.expect("a message"));
            }
        }
        assert_eq!(piecewise, whole);
    }

    #[test]
    fn a_split_inside_a_character_or_an_entity_does_not_matter() {
        let text = "<message device=\"Montaña\" message=\"a &amp; b — c\"/>";
        let whole = all(text);
        for split in 1..text.len() {
            let mut parser = StreamParser::new();
            let mut got = parser.feed(&text.as_bytes()[..split]);
            got.extend(parser.feed(&text.as_bytes()[split..]));
            let got: Vec<_> = got.into_iter().map(Result::unwrap).collect();
            assert_eq!(got, whole, "split at {split}");
        }
        assert_eq!(
            whole[0],
            IndiMessage::Message {
                device: Some("Montaña".into()),
                timestamp: None,
                text: "a & b — c".into(),
            }
        );
    }

    #[test]
    fn update_kinds_are_read() {
        let messages = all(concat!(
            r#"<setSwitchVector device="D" name="S" state="Ok"><oneSwitch name="A">On</oneSwitch></setSwitchVector>"#,
            r#"<setTextVector device="D" name="T"><oneText name="A"> hello &lt;there&gt; </oneText></setTextVector>"#,
            r#"<setLightVector device="D" name="L" state="Alert"><oneLight name="A">Alert</oneLight></setLightVector>"#,
        ));
        let values: Vec<_> = messages
            .iter()
            .map(|message| match message {
                IndiMessage::Update(update) => {
                    (update.kind, update.state, update.members[0].1.clone())
                }
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            values,
            [
                (
                    VectorKind::Switch,
                    Some(PropertyState::Ok),
                    MemberValue::Switch(SwitchState::On)
                ),
                // No state: it stays as it was. Text is trimmed and unescaped.
                (
                    VectorKind::Text,
                    None,
                    MemberValue::Text("hello <there>".into())
                ),
                (
                    VectorKind::Light,
                    Some(PropertyState::Alert),
                    MemberValue::Light(PropertyState::Alert)
                ),
            ]
        );
    }

    #[test]
    fn a_blob_wrapped_over_lines_is_decoded() {
        // "scopinator images" in base64, broken as indiserver breaks it.
        let text = "<setBLOBVector device=\"CCD\" name=\"CCD1\" state=\"Ok\">\n<oneBLOB name=\"CCD1\" size=\"17\" format=\".fits\">\nc2NvcGlu\nYXRvciBp\n  bWFnZXM=\n</oneBLOB>\n</setBLOBVector>";
        let messages = all(text);
        let IndiMessage::Update(update) = &messages[0] else {
            panic!("{:?}", messages[0]);
        };
        assert_eq!(
            update.members[0].1,
            MemberValue::Blob(Blob {
                format: ".fits".into(),
                data: b"scopinator images".to_vec(),
            })
        );
    }

    #[test]
    fn a_blob_of_the_wrong_size_or_not_base64_is_an_error() {
        let wrong_size = r#"<setBLOBVector device="C" name="CCD1"><oneBLOB name="CCD1" size="5" format=".fits">c2NvcGlu</oneBLOB></setBLOBVector>"#;
        let not_base64 = r#"<setBLOBVector device="C" name="CCD1"><oneBLOB name="CCD1" size="5" format=".fits">!!!!</oneBLOB></setBLOBVector>"#;
        for text in [wrong_size, not_base64] {
            let mut got = StreamParser::new().feed(text.as_bytes());
            assert!(matches!(got.pop(), Some(Err(IndiError::Blob(_)))), "{text}");
        }
        // Compressed data is as long as it is: `size` is for after.
        let compressed = r#"<setBLOBVector device="C" name="CCD1"><oneBLOB name="CCD1" size="5000" format=".fits.z">c2NvcGlu</oneBLOB></setBLOBVector>"#;
        assert!(StreamParser::new().feed(compressed.as_bytes())[0].is_ok());
    }

    #[test]
    fn a_bad_element_is_one_error_and_the_next_is_read() {
        let text = concat!(
            r#"<setNumberVector device="D" name="N"><oneNumber name="A">twelve</oneNumber></setNumberVector>"#,
            r#"<setNumberVector device="D" name="N"><oneNumber name="A">12</oneNumber></setNumberVector>"#,
        );
        let got = StreamParser::new().feed(text.as_bytes());
        assert_eq!(got.len(), 2);
        assert!(
            matches!(got[0], Err(IndiError::Malformed(_))),
            "{:?}",
            got[0]
        );
        assert!(got[1].is_ok());
    }

    #[test]
    fn garbage_between_elements_is_one_error() {
        let text = r#"<delProperty device="A"/> not xml at all </stray><delProperty device="B"/>"#;
        let got = StreamParser::new().feed(text.as_bytes());
        let kinds: Vec<_> = got
            .iter()
            .map(|message| match message {
                Ok(IndiMessage::Delete { device, .. }) => device.as_str(),
                Ok(_) => "other",
                Err(_) => "error",
            })
            .collect();
        // The text, then the closing tag with nothing open.
        assert_eq!(kinds, ["A", "error", "error", "B"]);
    }

    #[test]
    fn mismatched_tags_inside_an_element_fail_that_element_only() {
        let text = concat!(
            r#"<setTextVector device="D" name="T"><oneText name="A">x</oneNumber></setTextVector>"#,
            r#"<delProperty device="B"/>"#,
        );
        let got = StreamParser::new().feed(text.as_bytes());
        assert_eq!(got.len(), 2);
        assert!(got[0].is_err());
        assert!(got[1].is_ok());
    }

    #[test]
    fn what_is_not_known_is_not_an_error() {
        let messages = all(concat!(
            r#"<?xml version="1.0"?><!-- a comment with a > in it -->"#,
            r#"<pingRequest uid="7"/><newThing a="1"><inner>x</inner></newThing>"#,
            r#"<setTextVector device="D" name="T"><oneText name="A"><![CDATA[1 < 2]]></oneText><oneOther name="B">y</oneOther></setTextVector>"#,
        ));
        assert_eq!(
            messages[0],
            IndiMessage::Unknown {
                element: "pingRequest".into()
            }
        );
        assert_eq!(
            messages[1],
            IndiMessage::Unknown {
                element: "newThing".into()
            }
        );
        let IndiMessage::Update(update) = &messages[2] else {
            panic!("{:?}", messages[2]);
        };
        // The member of an unknown kind is left out.
        assert_eq!(
            update.members,
            [("A".to_string(), MemberValue::Text("1 < 2".into()))]
        );
    }

    #[test]
    fn a_greater_than_sign_inside_an_attribute_does_not_end_the_tag() {
        let messages = all(r#"<message device="D" message="a > b"/><delProperty device='E'/>"#);
        assert_eq!(messages.len(), 2);
        assert!(matches!(&messages[0], IndiMessage::Message { text, .. } if text == "a > b"));
    }

    #[test]
    fn a_property_missing_what_it_must_have_is_an_error() {
        for text in [
            r#"<defNumberVector name="N" state="Ok" perm="rw"/>"#,
            r#"<defNumberVector device="D" name="N" perm="rw"/>"#,
            r#"<defNumberVector device="D" name="N" state="Ok"/>"#,
            r#"<defNumberVector device="D" name="N" state="Sideways" perm="rw"/>"#,
            r#"<defSwitchVector device="D" name="N" state="Ok" perm="rw"><defSwitch name="A">Maybe</defSwitch></defSwitchVector>"#,
            r#"<delProperty name="N"/>"#,
        ] {
            let got = StreamParser::new().feed(text.as_bytes());
            assert!(
                matches!(got[..], [Err(IndiError::Malformed(_))]),
                "{text}: {got:?}"
            );
        }
    }

    #[test]
    fn an_element_that_is_too_large_is_given_up_on_and_the_stream_is_found_again() {
        let mut parser = StreamParser::with_max_element(4096);
        let mut text = String::from(
            r#"<setBLOBVector device="C" name="CCD1"><oneBLOB name="CCD1" format=".fits">"#,
        );
        text.push_str(&"QUJD".repeat(4000));
        text.push_str("</oneBLOB></setBLOBVector>");
        text.push_str(r#"<delProperty device="After"/>"#);

        let mut got = Vec::new();
        let mut most_held = 0;
        for chunk in text.as_bytes().chunks(700) {
            got.extend(parser.feed(chunk));
            most_held = most_held.max(parser.buf.len());
        }
        // Nothing near the size of the element was ever held.
        assert!(most_held <= 4096 + 700, "held {most_held} bytes");
        assert!(
            matches!(got[0], Err(IndiError::TooLarge { limit: 4096 })),
            "{:?}",
            got[0]
        );
        assert_eq!(
            got.last().unwrap().as_ref().unwrap(),
            &IndiMessage::Delete {
                device: "After".into(),
                name: None,
            }
        );
        assert_eq!(got.len(), 2, "{got:?}");
    }

    #[test]
    fn a_tag_that_never_ends_is_given_up_on() {
        let mut parser = StreamParser::new();
        let mut errors = 0;
        let chunk = vec![b'a'; 64 * 1024];
        errors += parser
            .feed(b"<message device=\"")
            .iter()
            .filter(|m| m.is_err())
            .count();
        for _ in 0..20 {
            errors += parser.feed(&chunk).iter().filter(|m| m.is_err()).count();
        }
        assert_eq!(errors, 1);
        assert!(parser.buf.len() <= MAX_TAG + chunk.len());
        // And it recovers.
        let got = parser.feed(br#""/><delProperty device="After"/>"#);
        assert!(
            matches!(got.last(), Some(Ok(IndiMessage::Delete { .. }))),
            "{got:?}"
        );
    }

    mod prop {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn any_split_gives_the_same_messages(cuts in proptest::collection::vec(0usize..SESSION.len(), 0..12)) {
                let whole = all(SESSION);
                let mut cuts = cuts;
                cuts.sort_unstable();
                let mut parser = StreamParser::new();
                let mut got = Vec::new();
                let mut from = 0;
                for cut in cuts.into_iter().chain([SESSION.len()]) {
                    for message in parser.feed(&SESSION.as_bytes()[from..cut]) {
                        got.push(message.expect("a message"));
                    }
                    from = cut;
                }
                prop_assert_eq!(got, whole);
            }

            #[test]
            fn arbitrary_bytes_never_panic(chunks in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..200), 0..8)) {
                let mut parser = StreamParser::with_max_element(512);
                for chunk in chunks {
                    let _ = parser.feed(&chunk);
                }
            }

            #[test]
            fn arbitrary_markup_never_panics(text in "[<>/!?\\[\\]\"'= a-zA-Z0-9&;-]{0,300}") {
                let mut parser = StreamParser::with_max_element(512);
                let _ = parser.feed(text.as_bytes());
            }
        }
    }
}
