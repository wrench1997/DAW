//! Closed, cooperative, allocation-accounted codec for the fresh-helper subset.
//!
//! No legacy serde audio/state decoder and no generic JSON value tree is used.
//! Encode counts before allocating one output Vec, then emits directly into it.
//! Decode first validates the complete response without heap allocations, then
//! materializes exactly that validated shape. Raw base64 is never copied: a small
//! streaming cursor also handles JSON escapes. The response line, final payload
//! capacities (including container storage), and request capacity are budgeted;
//! stack scratch, allocator metadata, caller storage and allocator/RSS behaviour
//! are not a hard memory bound. Failed codec transactions drop all their values
//! and restore their starting live charge; peak observations remain.

use super::{Budget, ErrorKind, ReplyShape, MAX_LINE};
use crate::audio::AudioBusBuffer;
use crate::discovery::ClassCompatibility;
use crate::plugin::{ProcessTransport, StateContext};
use crate::process_isolation::{HostCommand, HostResponse};
use std::fmt::Write as _;

const MAX_STATE: usize = 64 * 1024 * 1024 + 28;
const MAX_TEXT: usize = 64 * 1024;
const MAX_ID: usize = 64;
const MAX_BUSES: usize = 8;
const MAX_CHANNELS: usize = 32;
const MAX_FRAMES: usize = 4096;
const MAX_COMPAT: usize = 256;
const MAX_DEPTH: usize = 16;
// Canonical helper scalar spellings fit in 24 bytes. This explicitly bounded
// strict subset does not admit arbitrarily padded decimal/exponent spellings.
const MAX_NUMBER: usize = 128;
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

type Result<T> = std::result::Result<T, ErrorKind>;

fn limit(value: usize, maximum: usize) -> Result<()> {
    if value > maximum {
        Err(ErrorKind::Limit)
    } else {
        Ok(())
    }
}

fn base64_size(bytes: usize) -> Result<usize> {
    bytes
        .checked_add(2)
        .and_then(|n| (n / 3).checked_mul(4))
        .ok_or(ErrorKind::Limit)
}

fn check_shape(shape: &ReplyShape) -> Result<()> {
    match shape {
        ReplyShape::Audio { channels, frames } => {
            limit(*channels, MAX_CHANNELS)?;
            limit(*frames, MAX_FRAMES)
        }
        ReplyShape::Buses { buses, frames } => {
            limit(buses.len(), MAX_BUSES)?;
            limit(*frames, MAX_FRAMES)?;
            let mut total = 0usize;
            for &(_, channels) in buses {
                total = total.checked_add(channels).ok_or(ErrorKind::Limit)?;
                limit(total, MAX_CHANNELS)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn check_channels(channels: &[Vec<f32>], frames: usize) -> Result<()> {
    limit(channels.len(), MAX_CHANNELS)?;
    for channel in channels {
        limit(channel.len(), MAX_FRAMES)?;
        if channel.len() != frames {
            return Err(ErrorKind::Shape);
        }
    }
    Ok(())
}

fn check_transport(transport: &Option<ProcessTransport>) -> Result<()> {
    if let Some(t) = transport {
        if !t.quarter_note_position.is_finite()
            || !t.tempo.is_finite()
            || t.tempo <= 0.0
            || t.time_sig_numerator <= 0
            || !matches!(t.time_sig_denominator, 1 | 2 | 4 | 8 | 16)
        {
            return Err(ErrorKind::InvalidRequest);
        }
    }
    Ok(())
}

fn check_request(command: &HostCommand, shape: &ReplyShape) -> Result<()> {
    // Keep unsupported rejection first, before shape work or any allocation.
    match command {
        HostCommand::LoadPlugin { .. }
        | HostCommand::LoadState { .. }
        | HostCommand::SaveState
        | HostCommand::Process { .. }
        | HostCommand::ProcessBuses { .. }
        | HostCommand::GetParameter { .. }
        | HostCommand::LatencySamples
        | HostCommand::TailSamples => {}
        _ => return Err(ErrorKind::UnsupportedRequest),
    }
    check_shape(shape)?;
    match (command, shape) {
        (
            HostCommand::LoadPlugin {
                path,
                block_size,
                class_id,
                ..
            },
            ReplyShape::PluginInfo,
        ) => {
            limit(path.len(), MAX_TEXT)?;
            limit(*block_size as usize, MAX_FRAMES)?;
            if let Some(id) = class_id {
                limit(id.len(), MAX_ID)?;
            }
        }
        (HostCommand::LoadState { data, context }, ReplyShape::Success) => {
            limit(data.len(), MAX_STATE)?;
            limit(base64_size(data.len())?, MAX_LINE)?;
            if let StateContext::Preset { path: Some(path) } = context {
                limit(path.len(), MAX_TEXT)?;
            }
        }
        (HostCommand::SaveState, ReplyShape::State)
        | (HostCommand::GetParameter { .. }, ReplyShape::ParameterValue)
        | (HostCommand::LatencySamples, ReplyShape::LatencySamples)
        | (HostCommand::TailSamples, ReplyShape::TailSamples) => {}
        (
            HostCommand::Process {
                inputs,
                frames,
                transport,
            },
            ReplyShape::Audio {
                frames: expected, ..
            },
        ) => {
            if *frames as usize != *expected {
                return Err(ErrorKind::Shape);
            }
            check_channels(inputs, *expected)?;
            check_transport(transport)?;
        }
        (
            HostCommand::ProcessBuses {
                inputs,
                outputs,
                frames,
                transport,
            },
            ReplyShape::Buses {
                buses,
                frames: expected,
            },
        ) => {
            if *frames as usize != *expected || outputs.len() != buses.len() {
                return Err(ErrorKind::Shape);
            }
            limit(inputs.len(), MAX_BUSES)?;
            let mut total = 0usize;
            for bus in inputs {
                check_channels(&bus.channels, *expected)?;
                total = total
                    .checked_add(bus.channels.len())
                    .ok_or(ErrorKind::Limit)?;
                limit(total, MAX_CHANNELS)?;
            }
            for (output, &(active, channels)) in outputs.iter().zip(buses) {
                if output.active != active || output.channel_count != channels {
                    return Err(ErrorKind::Shape);
                }
            }
            check_transport(transport)?;
        }
        _ => return Err(ErrorKind::Shape),
    }
    Ok(())
}

struct Writer<'a> {
    bytes: Option<Vec<u8>>,
    len: usize,
    checked: usize,
    budget: &'a mut Budget,
}

impl Writer<'_> {
    fn put(&mut self, data: &[u8]) -> Result<()> {
        let end = self.len.checked_add(data.len()).ok_or(ErrorKind::Limit)?;
        limit(end, MAX_LINE)?;
        for chunk in data.chunks(1024) {
            if self.len - self.checked + chunk.len() >= 1024 {
                self.budget.check()?;
                self.checked = self.len;
            }
            if let Some(bytes) = &mut self.bytes {
                // The counting pass reserved exactly the entire request. Never
                // let Vec silently grow outside the payload-accounting path.
                if bytes.len() + chunk.len() > bytes.capacity() {
                    return Err(ErrorKind::Allocation);
                }
                bytes.extend_from_slice(chunk);
            }
            self.len += chunk.len();
        }
        debug_assert_eq!(self.len, end);
        Ok(())
    }

    fn boolean(&mut self, value: bool) -> Result<()> {
        self.put(if value { b"true" } else { b"false" })
    }

    fn number(&mut self, value: impl std::fmt::Display) -> Result<()> {
        struct Stack {
            bytes: [u8; 768],
            len: usize,
        }
        impl std::fmt::Write for Stack {
            fn write_str(&mut self, text: &str) -> std::fmt::Result {
                if text.len() > self.bytes.len() - self.len {
                    return Err(std::fmt::Error);
                }
                self.bytes[self.len..self.len + text.len()].copy_from_slice(text.as_bytes());
                self.len += text.len();
                Ok(())
            }
        }
        let mut stack = Stack {
            bytes: [0; 768],
            len: 0,
        };
        write!(&mut stack, "{value}").map_err(|_| ErrorKind::InvalidRequest)?;
        self.put(&stack.bytes[..stack.len])
    }

    fn float(&mut self, value: f64) -> Result<()> {
        if value.is_nan() {
            self.put(b"\"NaN\"")
        } else if value == f64::INFINITY {
            self.put(b"\"inf\"")
        } else if value == f64::NEG_INFINITY {
            self.put(b"\"-inf\"")
        } else if value == 0.0 && value.is_sign_negative() {
            self.put(b"-0.0")
        } else {
            self.number(format_args!("{value:e}"))
        }
    }

    fn text(&mut self, text: &str) -> Result<()> {
        self.put(b"\"")?;
        for chunk in text.as_bytes().chunks(1024) {
            self.budget.check()?;
            for &byte in chunk {
                match byte {
                    b'"' => self.put(b"\\\"")?,
                    b'\\' => self.put(b"\\\\")?,
                    0..=31 => {
                        const HEX: &[u8; 16] = b"0123456789abcdef";
                        self.put(&[
                            b'\\',
                            b'u',
                            b'0',
                            b'0',
                            HEX[(byte >> 4) as usize],
                            HEX[(byte & 15) as usize],
                        ])?;
                    }
                    _ => self.put(&[byte])?,
                }
            }
        }
        self.put(b"\"")
    }

    fn base64(&mut self, len: usize, mut byte_at: impl FnMut(usize) -> u8) -> Result<()> {
        // No raw sample Vec or intermediate base64 String is constructed.
        base64_size(len)?;
        self.put(b"\"")?;
        for start in (0..len).step_by(3) {
            let count = (len - start).min(3);
            let a = byte_at(start);
            let b = if count > 1 { byte_at(start + 1) } else { 0 };
            let c = if count > 2 { byte_at(start + 2) } else { 0 };
            self.put(&[
                ALPHABET[(a >> 2) as usize],
                ALPHABET[(((a & 3) << 4) | (b >> 4)) as usize],
                if count > 1 {
                    ALPHABET[(((b & 15) << 2) | (c >> 6)) as usize]
                } else {
                    b'='
                },
                if count > 2 {
                    ALPHABET[(c & 63) as usize]
                } else {
                    b'='
                },
            ])?;
        }
        self.put(b"\"")
    }

    fn channels(&mut self, channels: &[Vec<f32>]) -> Result<()> {
        self.put(b"[")?;
        for (index, channel) in channels.iter().enumerate() {
            if index != 0 {
                self.put(b",")?;
            }
            self.base64(
                channel.len().checked_mul(4).ok_or(ErrorKind::Limit)?,
                |byte| channel[byte / 4].to_bits().to_le_bytes()[byte % 4],
            )?;
        }
        self.put(b"]")
    }

    fn transport(&mut self, transport: &Option<ProcessTransport>) -> Result<()> {
        if let Some(t) = transport {
            self.put(b",\"transport\":{\"sample_position\":")?;
            self.number(t.sample_position)?;
            self.put(b",\"quarter_note_position\":")?;
            self.float(t.quarter_note_position)?;
            self.put(b",\"tempo\":")?;
            self.float(t.tempo)?;
            self.put(b",\"playing\":")?;
            self.boolean(t.playing)?;
            self.put(b",\"time_sig_numerator\":")?;
            self.number(t.time_sig_numerator)?;
            self.put(b",\"time_sig_denominator\":")?;
            self.number(t.time_sig_denominator)?;
            self.put(b"}")?;
        }
        Ok(())
    }

    fn command(&mut self, command: &HostCommand) -> Result<()> {
        match command {
            HostCommand::SaveState => self.put(b"\"SaveState\"")?,
            HostCommand::LatencySamples => self.put(b"\"LatencySamples\"")?,
            HostCommand::TailSamples => self.put(b"\"TailSamples\"")?,
            HostCommand::GetParameter { id } => {
                self.put(b"{\"GetParameter\":{\"id\":")?;
                self.number(id)?;
                self.put(b"}}")?;
            }
            HostCommand::LoadPlugin {
                path,
                sample_rate,
                block_size,
                tempo,
                time_sig_numerator,
                time_sig_denominator,
                class_id,
            } => {
                self.put(b"{\"LoadPlugin\":{\"path\":")?;
                self.text(path)?;
                self.put(b",\"sample_rate\":")?;
                self.float(*sample_rate)?;
                self.put(b",\"block_size\":")?;
                self.number(block_size)?;
                self.put(b",\"tempo\":")?;
                self.float(*tempo)?;
                self.put(b",\"time_sig_numerator\":")?;
                self.number(time_sig_numerator)?;
                self.put(b",\"time_sig_denominator\":")?;
                self.number(time_sig_denominator)?;
                self.put(b",\"class_id\":")?;
                match class_id {
                    Some(id) => self.text(id)?,
                    None => self.put(b"null")?,
                }
                self.put(b"}}")?;
            }
            HostCommand::LoadState { data, context } => {
                self.put(b"{\"LoadState\":{\"data\":")?;
                self.base64(data.len(), |index| data[index])?;
                self.put(b",\"context\":")?;
                match context {
                    StateContext::Project => self.put(b"\"Project\"")?,
                    StateContext::Preset { path } => {
                        self.put(b"{\"Preset\":{\"path\":")?;
                        match path {
                            Some(path) => self.text(path)?,
                            None => self.put(b"null")?,
                        }
                        self.put(b"}}")?;
                    }
                }
                self.put(b"}}")?;
            }
            HostCommand::Process {
                inputs,
                frames,
                transport,
            } => {
                self.put(b"{\"Process\":{\"inputs\":")?;
                self.channels(inputs)?;
                self.put(b",\"frames\":")?;
                self.number(frames)?;
                self.transport(transport)?;
                self.put(b"}}")?;
            }
            HostCommand::ProcessBuses {
                inputs,
                outputs,
                frames,
                transport,
            } => {
                self.put(b"{\"ProcessBuses\":{\"inputs\":[")?;
                for (index, bus) in inputs.iter().enumerate() {
                    if index != 0 {
                        self.put(b",")?;
                    }
                    self.put(b"{\"active\":")?;
                    self.boolean(bus.active)?;
                    self.put(b",\"channels\":")?;
                    self.channels(&bus.channels)?;
                    self.put(b"}")?;
                }
                self.put(b"],\"outputs\":[")?;
                for (index, bus) in outputs.iter().enumerate() {
                    if index != 0 {
                        self.put(b",")?;
                    }
                    self.put(b"{\"channel_count\":")?;
                    self.number(bus.channel_count)?;
                    self.put(b",\"active\":")?;
                    self.boolean(bus.active)?;
                    self.put(b"}")?;
                }
                self.put(b"],\"frames\":")?;
                self.number(frames)?;
                self.transport(transport)?;
                self.put(b"}}")?;
            }
            _ => return Err(ErrorKind::UnsupportedRequest),
        }
        self.put(b"\n")
    }
}

pub(super) fn encode(
    command: &HostCommand,
    shape: &ReplyShape,
    budget: &mut Budget,
) -> Result<Vec<u8>> {
    let starting_live = budget.live;
    let result = (|| {
        budget.check()?;
        check_request(command, shape)?;
        let mut count = Writer {
            bytes: None,
            len: 0,
            checked: 0,
            budget,
        };
        count.command(command)?;
        let length = count.len;
        let mut bytes = Vec::new();
        budget.grow(&mut bytes, length)?;
        let mut writer = Writer {
            bytes: Some(bytes),
            len: 0,
            checked: 0,
            budget,
        };
        writer.command(command)?;
        writer.budget.check()?;
        if writer.len != length {
            return Err(ErrorKind::InvalidRequest);
        }
        Ok(writer.bytes.take().expect("materializing request"))
    })();
    if result.is_err() {
        budget.live = starting_live;
    }
    result
}

#[derive(Clone, Copy)]
struct Token<'a> {
    raw: &'a [u8],
    decoded: usize,
}

// A single validated UTF-8/JSON string unit; at most twelve source bytes and
// four decoded bytes. This stack-only routine never traverses an unbounded span.
fn string_unit(raw: &[u8], position: usize) -> Result<([u8; 4], usize, usize)> {
    let byte = *raw.get(position).ok_or(ErrorKind::MalformedReply)?;
    let mut out = [0; 4];
    if byte == b'\\' {
        let escaped = *raw.get(position + 1).ok_or(ErrorKind::MalformedReply)?;
        out[0] = match escaped {
            b'"' => b'"',
            b'\\' => b'\\',
            b'/' => b'/',
            b'b' => 8,
            b'f' => 12,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'u' => {
                fn hex(raw: &[u8], start: usize) -> Result<u32> {
                    let bytes = raw.get(start..start + 4).ok_or(ErrorKind::MalformedReply)?;
                    let mut value = 0;
                    for byte in bytes {
                        value = value * 16
                            + u32::from(match byte {
                                b'0'..=b'9' => byte - b'0',
                                b'a'..=b'f' => byte - b'a' + 10,
                                b'A'..=b'F' => byte - b'A' + 10,
                                _ => return Err(ErrorKind::MalformedReply),
                            });
                    }
                    Ok(value)
                }
                let high = hex(raw, position + 2)?;
                let (scalar, consumed) = if (0xd800..=0xdbff).contains(&high) {
                    if raw.get(position + 6..position + 8) != Some(b"\\u") {
                        return Err(ErrorKind::MalformedReply);
                    }
                    let low = hex(raw, position + 8)?;
                    if !(0xdc00..=0xdfff).contains(&low) {
                        return Err(ErrorKind::MalformedReply);
                    }
                    (0x10000 + ((high - 0xd800) << 10) + low - 0xdc00, 12)
                } else {
                    (high, 6)
                };
                let character = char::from_u32(scalar).ok_or(ErrorKind::MalformedReply)?;
                let count = character.encode_utf8(&mut out).len();
                return Ok((out, count, consumed));
            }
            _ => return Err(ErrorKind::MalformedReply),
        };
        return Ok((out, 1, 2));
    }
    if byte < 32 || byte == b'"' {
        return Err(ErrorKind::MalformedReply);
    }
    let count = match byte {
        0x20..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return Err(ErrorKind::MalformedReply),
    };
    let bytes = raw
        .get(position..position + count)
        .ok_or(ErrorKind::MalformedReply)?;
    std::str::from_utf8(bytes).map_err(|_| ErrorKind::MalformedReply)?;
    out[..count].copy_from_slice(bytes);
    Ok((out, count, count))
}

struct TokenBytes<'a> {
    token: Token<'a>,
    position: usize,
    checked: usize,
    pending: [u8; 4],
    offset: usize,
    count: usize,
}
impl<'a> TokenBytes<'a> {
    fn new(token: Token<'a>) -> Self {
        Self {
            token,
            position: 0,
            checked: 0,
            pending: [0; 4],
            offset: 0,
            count: 0,
        }
    }
    fn next(&mut self, budget: &Budget) -> Result<Option<u8>> {
        if self.offset == self.count {
            if self.position == self.token.raw.len() {
                return Ok(None);
            }
            if self.position - self.checked >= 1024 {
                budget.check()?;
                self.checked = self.position;
            }
            let (bytes, count, consumed) = string_unit(self.token.raw, self.position)?;
            self.position += consumed;
            self.pending = bytes;
            self.offset = 0;
            self.count = count;
        }
        let value = self.pending[self.offset];
        self.offset += 1;
        Ok(Some(value))
    }
}

fn walk_base64(token: Token<'_>, budget: &Budget, mut emit: impl FnMut(&[u8])) -> Result<usize> {
    if token.decoded % 4 != 0 {
        return Err(ErrorKind::MalformedReply);
    }
    fn sextet(byte: u8) -> Result<u8> {
        Ok(match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(ErrorKind::MalformedReply),
        })
    }
    let mut cursor = TokenBytes::new(token);
    let mut decoded = 0usize;
    for group in 0..token.decoded / 4 {
        if group % 256 == 0 {
            budget.check()?;
        }
        let a = cursor.next(budget)?.ok_or(ErrorKind::MalformedReply)?;
        let b = cursor.next(budget)?.ok_or(ErrorKind::MalformedReply)?;
        let c = cursor.next(budget)?.ok_or(ErrorKind::MalformedReply)?;
        let d = cursor.next(budget)?.ok_or(ErrorKind::MalformedReply)?;
        let a = sextet(a)?;
        let b = sextet(b)?;
        let (c, d, count) = match (c, d) {
            (b'=', b'=') if b & 15 == 0 => (0, 0, 1),
            (c, b'=') if c != b'=' => {
                let c = sextet(c)?;
                if c & 3 != 0 {
                    return Err(ErrorKind::MalformedReply);
                }
                (c, 0, 2)
            }
            (c, d) => (sextet(c)?, sextet(d)?, 3),
        };
        if count != 3 && group + 1 != token.decoded / 4 {
            return Err(ErrorKind::MalformedReply);
        }
        let bytes = [(a << 2) | (b >> 4), (b << 4) | (c >> 2), (c << 6) | d];
        emit(&bytes[..count]);
        decoded = decoded.checked_add(count).ok_or(ErrorKind::Limit)?;
    }
    budget.check()?;
    Ok(decoded)
}

struct Key {
    bytes: [u8; 64],
    len: usize,
}
impl Key {
    fn is(&self, name: &[u8]) -> bool {
        &self.bytes[..self.len] == name
    }
}

struct Parser<'a, 'b> {
    bytes: &'a [u8],
    position: usize,
    checked: usize,
    depth: usize,
    materialize: bool,
    failure_message: Option<Token<'a>>,
    budget: &'b mut Budget,
}

impl<'a> Parser<'a, '_> {
    fn checkpoint(&mut self) -> Result<()> {
        if self.position - self.checked >= 1024 {
            self.budget.check()?;
            self.checked = self.position;
        }
        Ok(())
    }
    fn whitespace(&mut self) -> Result<()> {
        while matches!(
            self.bytes.get(self.position),
            Some(b' ' | b'\t' | b'\r' | b'\n')
        ) {
            self.position += 1;
            self.checkpoint()?;
        }
        Ok(())
    }
    fn peek(&mut self) -> Result<Option<u8>> {
        self.whitespace()?;
        Ok(self.bytes.get(self.position).copied())
    }
    fn take(&mut self, byte: u8) -> Result<()> {
        self.whitespace()?;
        if self.bytes.get(self.position) != Some(&byte) {
            return Err(ErrorKind::MalformedReply);
        }
        self.position += 1;
        self.checkpoint()
    }
    fn open(&mut self, byte: u8) -> Result<()> {
        self.take(byte)?;
        self.depth += 1;
        limit(self.depth, MAX_DEPTH)
    }
    fn next(&mut self, first: &mut bool, close: u8) -> Result<bool> {
        if self.peek()? == Some(close) {
            self.position += 1;
            self.depth = self.depth.checked_sub(1).ok_or(ErrorKind::MalformedReply)?;
            return Ok(false);
        }
        if !*first {
            self.take(b',')?;
        }
        *first = false;
        if self.peek()? == Some(close) {
            return Err(ErrorKind::MalformedReply);
        }
        Ok(true)
    }
    fn token(&mut self, maximum: usize) -> Result<Token<'a>> {
        self.take(b'"')?;
        let start = self.position;
        let mut decoded = 0usize;
        while self.bytes.get(self.position) != Some(&b'"') {
            let (_, count, consumed) = string_unit(self.bytes, self.position)?;
            decoded = decoded.checked_add(count).ok_or(ErrorKind::Limit)?;
            limit(decoded, maximum)?;
            self.position += consumed;
            self.checkpoint()?;
        }
        let token = Token {
            raw: &self.bytes[start..self.position],
            decoded,
        };
        self.position += 1;
        self.checkpoint()?;
        Ok(token)
    }
    fn key(&mut self) -> Result<Key> {
        let token = self.token(64)?;
        let mut key = Key {
            bytes: [0; 64],
            len: 0,
        };
        let mut cursor = TokenBytes::new(token);
        while let Some(byte) = cursor.next(self.budget)? {
            key.bytes[key.len] = byte;
            key.len += 1;
        }
        self.take(b':')?;
        Ok(key)
    }
    fn text(&mut self, maximum: usize) -> Result<String> {
        let token = self.token(maximum)?;
        if !self.materialize {
            return Ok(String::new());
        }
        owned_text(token, self.budget)
    }

    fn literal(&mut self, literal: &[u8]) -> Result<()> {
        self.whitespace()?;
        if self.bytes.get(self.position..self.position + literal.len()) != Some(literal) {
            return Err(ErrorKind::MalformedReply);
        }
        self.position += literal.len();
        self.checkpoint()
    }
    fn boolean(&mut self) -> Result<bool> {
        match self.peek()? {
            Some(b't') => {
                self.literal(b"true")?;
                Ok(true)
            }
            Some(b'f') => {
                self.literal(b"false")?;
                Ok(false)
            }
            _ => Err(ErrorKind::MalformedReply),
        }
    }
    fn number(&mut self) -> Result<&'a str> {
        self.whitespace()?;
        let start = self.position;
        while matches!(
            self.bytes.get(self.position),
            Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
        ) {
            self.position += 1;
            limit(self.position - start, MAX_NUMBER)?;
            self.checkpoint()?;
        }
        let bytes = &self.bytes[start..self.position];
        // Validate the JSON grammar independently of Rust's permissive float parser.
        let mut i = 0;
        if bytes.get(i) == Some(&b'-') {
            i += 1;
        }
        match bytes.get(i) {
            Some(b'0') => i += 1,
            Some(b'1'..=b'9') => {
                i += 1;
                while matches!(bytes.get(i), Some(b'0'..=b'9')) {
                    i += 1;
                }
            }
            _ => return Err(ErrorKind::MalformedReply),
        }
        if bytes.get(i) == Some(&b'.') {
            i += 1;
            let first = i;
            while matches!(bytes.get(i), Some(b'0'..=b'9')) {
                i += 1;
            }
            if i == first {
                return Err(ErrorKind::MalformedReply);
            }
        }
        if matches!(bytes.get(i), Some(b'e' | b'E')) {
            i += 1;
            if matches!(bytes.get(i), Some(b'+' | b'-')) {
                i += 1;
            }
            let first = i;
            while matches!(bytes.get(i), Some(b'0'..=b'9')) {
                i += 1;
            }
            if i == first {
                return Err(ErrorKind::MalformedReply);
            }
        }
        if i != bytes.len() {
            return Err(ErrorKind::MalformedReply);
        }
        std::str::from_utf8(bytes).map_err(|_| ErrorKind::MalformedReply)
    }
    fn u32(&mut self) -> Result<u32> {
        self.number()?
            .parse()
            .map_err(|_| ErrorKind::MalformedReply)
    }
    fn count(&mut self, maximum: usize) -> Result<i32> {
        let count: i32 = self
            .number()?
            .parse()
            .map_err(|_| ErrorKind::MalformedReply)?;
        if count < 0 {
            return Err(ErrorKind::Limit);
        }
        limit(count as usize, maximum)?;
        Ok(count)
    }
    fn float(&mut self) -> Result<f64> {
        if self.peek()? == Some(b'"') {
            let token = self.token(4)?;
            let mut bytes = [0u8; 4];
            let mut cursor = TokenBytes::new(token);
            for byte in &mut bytes[..token.decoded] {
                *byte = cursor.next(self.budget)?.ok_or(ErrorKind::MalformedReply)?;
            }
            return match &bytes[..token.decoded] {
                b"NaN" => Ok(f64::NAN),
                b"inf" => Ok(f64::INFINITY),
                b"-inf" => Ok(f64::NEG_INFINITY),
                _ => Err(ErrorKind::MalformedReply),
            };
        }
        // Legacy null loses value class and is deliberately not accepted here.
        let value: f64 = self
            .number()?
            .parse()
            .map_err(|_| ErrorKind::MalformedReply)?;
        if !value.is_finite() {
            return Err(ErrorKind::MalformedReply);
        }
        Ok(value)
    }
    fn state(&mut self) -> Result<Vec<u8>> {
        if self.peek()? == Some(b'[') {
            return Err(ErrorKind::UnsupportedReply);
        }
        let token = self.token(base64_size(MAX_STATE)?)?;
        let length = walk_base64(token, self.budget, |_| {})?;
        limit(length, MAX_STATE)?;
        let mut state = Vec::new();
        if self.materialize {
            self.budget.grow(&mut state, length)?;
            walk_base64(token, self.budget, |bytes| state.extend_from_slice(bytes))?;
        }
        Ok(state)
    }
    fn channel(&mut self, frames: usize) -> Result<Vec<f32>> {
        let token = self.token(base64_size(MAX_FRAMES * 4)?)?;
        let length = walk_base64(token, self.budget, |_| {})?;
        if length % 4 != 0 {
            return Err(ErrorKind::MalformedReply);
        }
        if length != frames * 4 {
            return Err(ErrorKind::Shape);
        }
        let mut samples = Vec::new();
        if self.materialize {
            self.budget.grow(&mut samples, frames)?;
            let mut sample = [0u8; 4];
            let mut used = 0;
            walk_base64(token, self.budget, |bytes| {
                for byte in bytes {
                    sample[used] = *byte;
                    used += 1;
                    if used == 4 {
                        samples.push(f32::from_bits(u32::from_le_bytes(sample)));
                        used = 0;
                    }
                }
            })?;
        }
        Ok(samples)
    }
    fn channels(&mut self, expected: usize, frames: usize) -> Result<Vec<Vec<f32>>> {
        self.open(b'[')?;
        let mut channels = Vec::new();
        if self.materialize {
            self.budget.grow(&mut channels, expected)?;
        }
        let mut first = true;
        let mut count = 0;
        while self.next(&mut first, b']')? {
            if count == expected {
                return Err(ErrorKind::Shape);
            }
            let channel = self.channel(frames)?;
            if self.materialize {
                channels.push(channel);
            }
            count += 1;
        }
        if count != expected {
            return Err(ErrorKind::Shape);
        }
        Ok(channels)
    }
    fn buses(&mut self, expected: &[(bool, usize)], frames: usize) -> Result<Vec<AudioBusBuffer>> {
        self.open(b'[')?;
        let mut buses = Vec::new();
        if self.materialize {
            self.budget.grow(&mut buses, expected.len())?;
        }
        let mut first = true;
        let mut count = 0;
        while self.next(&mut first, b']')? {
            let &(expected_active, expected_channels) =
                expected.get(count).ok_or(ErrorKind::Shape)?;
            self.open(b'{')?;
            let mut fields = true;
            let mut seen = 0;
            let mut active = false;
            let mut channels = Vec::new();
            while self.next(&mut fields, b'}')? {
                let key = self.key()?;
                if key.is(b"active") {
                    unique(&mut seen, 1)?;
                    active = self.boolean()?;
                } else if key.is(b"channels") {
                    unique(&mut seen, 2)?;
                    channels = self.channels(expected_channels, frames)?;
                } else {
                    return Err(ErrorKind::MalformedReply);
                }
            }
            required(seen, 3)?;
            if active != expected_active {
                return Err(ErrorKind::Shape);
            }
            if self.materialize {
                buses.push(AudioBusBuffer { active, channels });
            }
            count += 1;
        }
        if count != expected.len() {
            return Err(ErrorKind::Shape);
        }
        Ok(buses)
    }
    fn no_events(&mut self) -> Result<()> {
        self.open(b'[')?;
        // Examine only the next token. No element payload, JSON nesting, event
        // kind or potentially enormous pointer-backed data is read or decoded.
        if self.peek()? != Some(b']') {
            return Err(ErrorKind::UnsupportedReply);
        }
        self.take(b']')?;
        self.depth -= 1;
        Ok(())
    }
    fn compatibility(&mut self) -> Result<Vec<ClassCompatibility>> {
        self.open(b'[')?;
        let mut entries = Vec::new();
        let mut first = true;
        let mut count = 0usize;
        let mut old_total = 0usize;
        while self.next(&mut first, b']')? {
            if count == MAX_COMPAT {
                return Err(ErrorKind::Limit);
            }
            self.open(b'{')?;
            let mut fields = true;
            let mut seen = 0;
            let mut new_class_id = String::new();
            let mut old_class_ids = Vec::new();
            while self.next(&mut fields, b'}')? {
                let key = self.key()?;
                if key.is(b"new_class_id") {
                    unique(&mut seen, 1)?;
                    new_class_id = self.text(MAX_ID)?;
                } else if key.is(b"old_class_ids") {
                    unique(&mut seen, 2)?;
                    self.open(b'[')?;
                    let mut ids_first = true;
                    let mut old_count = 0;
                    while self.next(&mut ids_first, b']')? {
                        if old_count == MAX_COMPAT || old_total == MAX_COMPAT {
                            return Err(ErrorKind::Limit);
                        }
                        let old = self.text(MAX_ID)?;
                        if self.materialize {
                            self.budget.grow(&mut old_class_ids, old_count + 1)?;
                            old_class_ids.push(old);
                        }
                        old_count += 1;
                        old_total += 1;
                    }
                } else {
                    return Err(ErrorKind::MalformedReply);
                }
            }
            required(seen, 3)?;
            if self.materialize {
                self.budget.grow(&mut entries, count + 1)?;
                entries.push(ClassCompatibility {
                    new_class_id,
                    old_class_ids,
                });
            }
            count += 1;
        }
        Ok(entries)
    }
    fn plugin_info(&mut self) -> Result<HostResponse> {
        self.open(b'{')?;
        let mut first = true;
        let mut seen = 0;
        let (mut vendor, mut name, mut version, mut category, mut uid) = (
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        );
        let (mut has_gui, mut has_midi_input, mut has_midi_output) = (false, false, false);
        let (mut audio_inputs, mut audio_outputs, mut output_channels) = (0, 0, 0);
        let mut compatibility = Vec::new();
        while self.next(&mut first, b'}')? {
            let key = self.key()?;
            if key.is(b"vendor") {
                unique(&mut seen, 1)?;
                vendor = self.text(MAX_TEXT)?;
            } else if key.is(b"name") {
                unique(&mut seen, 2)?;
                name = self.text(MAX_TEXT)?;
            } else if key.is(b"version") {
                unique(&mut seen, 4)?;
                version = self.text(MAX_TEXT)?;
            } else if key.is(b"category") {
                unique(&mut seen, 8)?;
                category = self.text(MAX_TEXT)?;
            } else if key.is(b"uid") {
                unique(&mut seen, 16)?;
                uid = self.text(MAX_ID)?;
            } else if key.is(b"has_gui") {
                unique(&mut seen, 32)?;
                has_gui = self.boolean()?;
            } else if key.is(b"audio_inputs") {
                unique(&mut seen, 64)?;
                audio_inputs = self.count(MAX_BUSES)?;
            } else if key.is(b"audio_outputs") {
                unique(&mut seen, 128)?;
                audio_outputs = self.count(MAX_BUSES)?;
            } else if key.is(b"output_channels") {
                unique(&mut seen, 256)?;
                output_channels = self.count(MAX_CHANNELS)?;
            } else if key.is(b"has_midi_input") {
                unique(&mut seen, 512)?;
                has_midi_input = self.boolean()?;
            } else if key.is(b"has_midi_output") {
                unique(&mut seen, 1024)?;
                has_midi_output = self.boolean()?;
            } else if key.is(b"compatibility") {
                unique(&mut seen, 2048)?;
                compatibility = self.compatibility()?;
            } else {
                return Err(ErrorKind::MalformedReply);
            }
        }
        // The existing wire explicitly defaults compatibility; all other fields
        // are required. Never clamp out-of-range bus/channel metadata.
        required(seen, 2047)?;
        Ok(HostResponse::PluginInfo {
            vendor,
            name,
            version,
            category,
            uid,
            has_gui,
            audio_inputs,
            audio_outputs,
            output_channels,
            has_midi_input,
            has_midi_output,
            compatibility,
        })
    }
    fn response(&mut self, shape: &ReplyShape) -> Result<HostResponse> {
        self.open(b'{')?;
        let mut outer = true;
        if !self.next(&mut outer, b'}')? {
            return Err(ErrorKind::MalformedReply);
        }
        let variant = self.key()?;
        let reply = if variant.is(b"PluginInfo") {
            if *shape != ReplyShape::PluginInfo {
                return Err(ErrorKind::WrongReply);
            }
            self.plugin_info()?
        } else if variant.is(b"Error") || variant.is(b"Crashed") || variant.is(b"Success") {
            if variant.is(b"Success") && *shape != ReplyShape::Success {
                return Err(ErrorKind::WrongReply);
            }
            self.open(b'{')?;
            let mut first = true;
            let mut seen = 0;
            let mut message = String::new();
            while self.next(&mut first, b'}')? {
                if !self.key()?.is(b"message") {
                    return Err(ErrorKind::MalformedReply);
                }
                unique(&mut seen, 1)?;
                if variant.is(b"Error") || variant.is(b"Crashed") {
                    let token = self.token(MAX_TEXT)?;
                    self.failure_message = Some(token);
                    if self.materialize {
                        message = owned_text(token, self.budget)?;
                    }
                } else {
                    message = self.text(MAX_TEXT)?;
                }
            }
            required(seen, 1)?;
            if variant.is(b"Error") {
                HostResponse::Error { message }
            } else if variant.is(b"Crashed") {
                HostResponse::Crashed { message }
            } else {
                HostResponse::Success { message }
            }
        } else if variant.is(b"AudioOutput") || variant.is(b"BusAudioOutput") {
            let bus_response = variant.is(b"BusAudioOutput");
            match (bus_response, shape) {
                (false, ReplyShape::Audio { .. }) | (true, ReplyShape::Buses { .. }) => {}
                _ => return Err(ErrorKind::WrongReply),
            }
            self.open(b'{')?;
            let mut first = true;
            let mut seen = 0;
            let mut outputs = Vec::new();
            let mut bus_outputs = Vec::new();
            let mut output_events_lost = false;
            let mut transport_applied = false;
            while self.next(&mut first, b'}')? {
                let key = self.key()?;
                if key.is(b"outputs") {
                    unique(&mut seen, 1)?;
                    match shape {
                        ReplyShape::Audio { channels, frames } => {
                            outputs = self.channels(*channels, *frames)?
                        }
                        ReplyShape::Buses { buses, frames } => {
                            bus_outputs = self.buses(buses, *frames)?
                        }
                        _ => return Err(ErrorKind::WrongReply),
                    }
                } else if key.is(b"output_events") {
                    unique(&mut seen, 2)?;
                    self.no_events()?;
                } else if key.is(b"output_events_lost") {
                    unique(&mut seen, 4)?;
                    output_events_lost = self.boolean()?;
                } else if key.is(b"transport_applied") {
                    unique(&mut seen, 8)?;
                    transport_applied = self.boolean()?;
                } else {
                    return Err(ErrorKind::MalformedReply);
                }
            }
            // These two flags have explicit legacy false defaults in HostResponse.
            required(seen, 3)?;
            if bus_response {
                HostResponse::BusAudioOutput {
                    outputs: bus_outputs,
                    output_events: Vec::new(),
                    output_events_lost,
                    transport_applied,
                }
            } else {
                HostResponse::AudioOutput {
                    outputs,
                    output_events: Vec::new(),
                    output_events_lost,
                    transport_applied,
                }
            }
        } else if variant.is(b"State") {
            if *shape != ReplyShape::State {
                return Err(ErrorKind::WrongReply);
            }
            self.open(b'{')?;
            let mut first = true;
            let mut seen = 0;
            let mut data = Vec::new();
            while self.next(&mut first, b'}')? {
                if !self.key()?.is(b"data") {
                    return Err(ErrorKind::MalformedReply);
                }
                unique(&mut seen, 1)?;
                data = self.state()?;
            }
            required(seen, 1)?;
            HostResponse::State { data }
        } else if variant.is(b"ParameterValue") {
            if *shape != ReplyShape::ParameterValue {
                return Err(ErrorKind::WrongReply);
            }
            self.open(b'{')?;
            let mut first = true;
            let mut seen = 0;
            let mut value = 0.0;
            while self.next(&mut first, b'}')? {
                if !self.key()?.is(b"value") {
                    return Err(ErrorKind::MalformedReply);
                }
                unique(&mut seen, 1)?;
                value = self.float()?;
            }
            required(seen, 1)?;
            HostResponse::ParameterValue { value }
        } else if variant.is(b"LatencySamples") || variant.is(b"TailSamples") {
            if !((variant.is(b"LatencySamples") && *shape == ReplyShape::LatencySamples)
                || (variant.is(b"TailSamples") && *shape == ReplyShape::TailSamples))
            {
                return Err(ErrorKind::WrongReply);
            }
            self.open(b'{')?;
            let mut first = true;
            let mut seen = 0;
            let mut samples = 0;
            while self.next(&mut first, b'}')? {
                if !self.key()?.is(b"samples") {
                    return Err(ErrorKind::MalformedReply);
                }
                unique(&mut seen, 1)?;
                samples = self.u32()?;
            }
            required(seen, 1)?;
            if variant.is(b"LatencySamples") {
                HostResponse::LatencySamples { samples }
            } else {
                HostResponse::TailSamples { samples }
            }
        } else {
            return Err(ErrorKind::UnsupportedReply);
        };
        if self.next(&mut outer, b'}')? {
            return Err(ErrorKind::MalformedReply);
        }
        self.whitespace()?;
        if self.position != self.bytes.len() || self.depth != 0 {
            return Err(ErrorKind::MalformedReply);
        }
        self.budget.check()?;
        Ok(reply)
    }
}

fn unique(seen: &mut u32, bit: u32) -> Result<()> {
    if *seen & bit != 0 {
        return Err(ErrorKind::MalformedReply);
    }
    *seen |= bit;
    Ok(())
}
fn required(seen: u32, fields: u32) -> Result<()> {
    if seen & fields != fields {
        Err(ErrorKind::MalformedReply)
    } else {
        Ok(())
    }
}

/// A known helper failure is distinct from an expected successful reply: after
/// its complete checked preflight, late cancellation cannot erase the category.
#[derive(Debug)]
pub(super) enum DecodedReply {
    Response(HostResponse),
    KnownFailure {
        crashed: bool,
        reported: Option<HostResponse>,
        diagnostic_unavailable: Option<ErrorKind>,
    },
}

fn owned_text(token: Token<'_>, budget: &mut Budget) -> Result<String> {
    let mut bytes = Vec::new();
    budget.grow(&mut bytes, token.decoded)?;
    // Empty conversion preserves the already-accounted Vec capacity without
    // performing an uncheckpointed full-string UTF-8 validation or any unsafe.
    let mut text = String::from_utf8(bytes).map_err(|_| ErrorKind::MalformedReply)?;
    let mut position = 0usize;
    let mut checked = 0usize;
    while position < token.raw.len() {
        if position - checked >= 1024 {
            budget.check()?;
            checked = position;
        }
        let (unit, count, consumed) = string_unit(token.raw, position)?;
        let unit = std::str::from_utf8(&unit[..count]).map_err(|_| ErrorKind::MalformedReply)?;
        text.push_str(unit);
        position += consumed;
    }
    budget.check()?;
    Ok(text)
}

// Only preflight constructs this marker, after complete syntax/schema/length
// validation and its final cancellation/deadline check. It retains only the
// bounded diagnostic token, never an instruction to reparse the full line.
struct KnownOperationFailure<'a> {
    crashed: bool,
    message: Token<'a>,
}
impl KnownOperationFailure<'_> {
    fn materialize(self, budget: &mut Budget) -> DecodedReply {
        let starting_live = budget.live;
        let mut diagnostic = Budget {
            deadline: budget.deadline,
            cancel: budget.cancel.clone(),
            live: starting_live,
            peak: budget.peak,
            diagnostic_copy: true,
        };
        // This failure-only tail copies <=64 KiB decoded text (<=6 input bytes
        // per decoded byte). The original normal budget is never relaxed.
        let result = owned_text(self.message, &mut diagnostic);
        budget.peak = budget.peak.max(diagnostic.peak);
        match result {
            Ok(message) => {
                budget.live = diagnostic.live;
                let reported = if self.crashed {
                    HostResponse::Crashed { message }
                } else {
                    HostResponse::Error { message }
                };
                DecodedReply::KnownFailure {
                    crashed: self.crashed,
                    reported: Some(reported),
                    diagnostic_unavailable: None,
                }
            }
            Err(error) => {
                // The failed String/Vec has already been dropped. Allocation or
                // limit failure can lose the detail, never the known category.
                budget.live = starting_live;
                DecodedReply::KnownFailure {
                    crashed: self.crashed,
                    reported: None,
                    diagnostic_unavailable: Some(error),
                }
            }
        }
    }
}

fn preflight<'a>(
    bytes: &'a [u8],
    shape: &ReplyShape,
    budget: &mut Budget,
) -> Result<Option<KnownOperationFailure<'a>>> {
    budget.check()?;
    limit(bytes.len(), MAX_LINE)?;
    check_shape(shape)?;
    let mut parser = Parser {
        bytes,
        position: 0,
        checked: 0,
        depth: 0,
        materialize: false,
        failure_message: None,
        budget,
    };
    let validated = parser.response(shape)?;
    parser.budget.check()?;
    let crashed = match validated {
        HostResponse::Error { .. } => false,
        HostResponse::Crashed { .. } => true,
        _ => return Ok(None),
    };
    Ok(Some(KnownOperationFailure {
        crashed,
        message: parser.failure_message.ok_or(ErrorKind::MalformedReply)?,
    }))
}

pub(super) fn decode(
    bytes: &[u8],
    shape: &ReplyShape,
    budget: &mut Budget,
) -> Result<DecodedReply> {
    let starting_live = budget.live;
    let result = (|| {
        if let Some(failure) = preflight(bytes, shape, budget)? {
            return Ok(failure.materialize(budget));
        }
        // No final payload allocation occurred until exact shape, required fields,
        // full base64 validity and the entire closed response were established.
        let mut parser = Parser {
            bytes,
            position: 0,
            checked: 0,
            depth: 0,
            materialize: true,
            failure_message: None,
            budget,
        };
        let response = parser.response(shape)?;
        parser.budget.check()?;
        Ok(DecodedReply::Response(response))
    })();
    if result.is_err() {
        budget.live = starting_live;
    }
    result
}

#[cfg(test)]
#[path = "codec_tests.rs"]
mod tests;
