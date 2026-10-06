use std::collections::BTreeSet;
use std::io::Read;
use std::ops::Range;

use anyhow::{Context, ensure};

use super::MAX_SOURCE_BYTES;
use crate::secure::RelativeSafePath;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Add,
    Delete,
    Modify,
    Copy,
    Rename,
}

#[derive(Debug)]
pub(super) struct Operation {
    pub group: usize,
    pub source: Option<String>,
    pub destination: Option<String>,
    pub kind: Kind,
    pub old_oid: Option<String>,
    pub new_oid: Option<String>,
    pub old_mode: Option<String>,
    pub new_mode: Option<String>,
    pub range: Range<usize>,
    added: usize,
    removed: usize,
    binary: Option<usize>,
}

impl Operation {
    pub fn paths(&self) -> Vec<String> {
        self.source
            .iter()
            .chain(&self.destination)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn output_bound(&self, source: usize, exact: bool, limit: usize) -> anyhow::Result<usize> {
        let result = if self.kind == Kind::Delete {
            0
        } else if let Some(size) = self.binary {
            size
        } else if exact {
            source
                .checked_sub(self.removed)
                .and_then(|size| size.checked_add(self.added))
                .context("Patch removal/growth is inconsistent with its preimage")?
        } else {
            source
                .checked_add(self.added)
                .context("Patch growth overflow")?
        };
        ensure!(
            result <= limit,
            "PR pre-application expansion exceeds the configured content bound"
        );
        Ok(result)
    }
}

pub(super) struct Document<'a> {
    text: &'a [u8],
    pub operations: Vec<Operation>,
}

impl<'a> Document<'a> {
    pub fn parse(text: &'a [u8]) -> anyhow::Result<Self> {
        let mut offset = 0;
        let lines = text
            .split_inclusive(|byte| *byte == b'\n')
            .map(|raw| {
                let start = offset;
                offset += raw.len();
                (start, raw)
            })
            .collect::<Vec<_>>();
        let mut operations = Vec::new();
        let mut i = 0;
        let mut binary_work = 0usize;
        let mut group = 0usize;
        while i < lines.len() {
            let (start, raw) = lines[i];
            let line = header_line(raw)?;
            let Some(header) = line.strip_prefix("diff --git ") else {
                if let Some(envelope) = line.strip_prefix("From ")
                    && envelope
                        .strip_suffix(" Mon Sep 17 00:00:00 2001")
                        .is_some_and(|oid| {
                            oid.len() == 40 && oid.bytes().all(|byte| byte.is_ascii_hexdigit())
                        })
                {
                    group += 1;
                }
                ensure!(
                    !line.starts_with("--- ")
                        && !line.starts_with("+++ ")
                        && !line.starts_with("@@ "),
                    "Patch data appears outside a supported diff --git block"
                );
                i += 1;
                continue;
            };
            i += 1;
            let mut source = None;
            let mut destination = None;
            let mut source_seen = false;
            let mut destination_seen = false;
            let mut movement: Option<Kind> = None;
            let mut movement_source = false;
            let mut movement_destination = false;
            let mut old_oid = None;
            let mut new_oid = None;
            let mut old_mode = None;
            let mut new_mode = None;
            let mut created = false;
            let mut deleted = false;
            let mut added = 0usize;
            let mut removed = 0usize;
            let mut binary = None;
            let mut hunks = false;
            let mut end = start + raw.len();
            while i < lines.len() {
                let (position, raw) = lines[i];
                let line = header_line(raw)?;
                if line.starts_with("diff --git ") {
                    break;
                }
                if line.starts_with("@@ ") {
                    let (mut old, mut new) = hunk_counts(line)?;
                    hunks = true;
                    i += 1;
                    let mut previous = None;
                    while i < lines.len() {
                        let (position, raw) = lines[i];
                        if strip_eol(raw) == br"\ No newline at end of file" {
                            match previous.take() {
                                Some(b'+') => {
                                    added =
                                        added.checked_sub(1).context("Invalid newline marker")?
                                }
                                Some(b'-') => {
                                    removed =
                                        removed.checked_sub(1).context("Invalid newline marker")?
                                }
                                Some(b' ') => {}
                                _ => anyhow::bail!("Unattached or repeated patch newline marker"),
                            }
                            end = position + raw.len();
                            i += 1;
                            continue;
                        }
                        if old == 0 && new == 0 {
                            break;
                        }
                        let sign = *raw.first().context("Empty patch hunk line")?;
                        match sign {
                            b' ' => {
                                old = old
                                    .checked_sub(1)
                                    .context("Patch old hunk count mismatch")?;
                                new = new
                                    .checked_sub(1)
                                    .context("Patch new hunk count mismatch")?;
                            }
                            b'-' => {
                                old = old
                                    .checked_sub(1)
                                    .context("Patch old hunk count mismatch")?;
                                removed = removed
                                    .checked_add(raw.len() - 1)
                                    .context("Patch removal overflow")?;
                            }
                            b'+' => {
                                new = new
                                    .checked_sub(1)
                                    .context("Patch new hunk count mismatch")?;
                                added = added
                                    .checked_add(raw.len() - 1)
                                    .context("Patch growth overflow")?;
                            }
                            _ => anyhow::bail!("Malformed patch hunk"),
                        }
                        previous = raw.ends_with(b"\n").then_some(sign);
                        end = position + raw.len();
                        i += 1;
                    }
                    ensure!(old == 0 && new == 0, "Truncated patch hunk");
                    continue;
                }
                if line == "GIT binary patch" {
                    ensure!(
                        !hunks && binary.is_none(),
                        "Conflicting binary/text patch bodies"
                    );
                    i += 1;
                    let mut frames = 0;
                    while i < lines.len() {
                        let frame = header_line(lines[i].1)?;
                        let Some((kind, size)) = frame.split_once(' ') else {
                            break;
                        };
                        if !matches!(kind, "literal" | "delta") {
                            break;
                        }
                        let declared: usize = size.parse().context("Invalid binary frame size")?;
                        ensure!(
                            declared <= MAX_SOURCE_BYTES,
                            "Binary frame exceeds source-processing bound"
                        );
                        i += 1;
                        let mut compressed = Vec::new();
                        while i < lines.len() && !strip_eol(lines[i].1).is_empty() {
                            decode_binary_line(header_line(lines[i].1)?, &mut compressed)?;
                            ensure!(
                                compressed.len() <= MAX_SOURCE_BYTES,
                                "Compressed binary frame exceeds bound"
                            );
                            end = lines[i].0 + lines[i].1.len();
                            i += 1;
                        }
                        if i < lines.len() {
                            end = lines[i].0 + lines[i].1.len();
                            i += 1;
                        }
                        let mut decoded = Vec::new();
                        flate2::read::ZlibDecoder::new(compressed.as_slice())
                            .take(u64::try_from(MAX_SOURCE_BYTES)? + 1)
                            .read_to_end(&mut decoded)
                            .context("Invalid binary patch compression")?;
                        ensure!(
                            decoded.len() == declared,
                            "Binary frame decompression size mismatch"
                        );
                        let result = if kind == "literal" {
                            declared
                        } else {
                            let mut cursor = 0;
                            let source = delta_size(&decoded, &mut cursor)?;
                            let result = delta_size(&decoded, &mut cursor)?;
                            ensure!(
                                source <= MAX_SOURCE_BYTES && result <= MAX_SOURCE_BYTES,
                                "Binary delta exceeds source-processing bound"
                            );
                            result
                        };
                        binary_work = binary_work
                            .checked_add(result)
                            .context("Binary expansion overflow")?;
                        ensure!(
                            binary_work <= MAX_SOURCE_BYTES,
                            "Aggregate binary pre-application expansion exceeds 10 MiB"
                        );
                        if frames == 0 {
                            binary = Some(result);
                        }
                        frames += 1;
                        ensure!(frames <= 2, "Unexpected binary patch frame");
                    }
                    ensure!(frames > 0, "Missing binary patch body");
                    continue;
                }
                if hunks || binary.is_some() {
                    ensure!(
                        ![
                            "--- ",
                            "+++ ",
                            "copy ",
                            "rename ",
                            "index ",
                            "new file mode ",
                            "deleted file mode ",
                            "old mode ",
                            "new mode "
                        ]
                        .iter()
                        .any(|prefix| line.starts_with(prefix)),
                        "Path metadata follows patch content"
                    );
                    break;
                }
                if let Some(value) = line.strip_prefix("--- ") {
                    ensure!(!source_seen, "Duplicate old patch path");
                    let path = patch_path(value, "a/")?;
                    ensure!(
                        source.is_none() || source == path,
                        "Conflicting old patch paths"
                    );
                    source = path;
                    source_seen = true;
                } else if let Some(value) = line.strip_prefix("+++ ") {
                    ensure!(!destination_seen, "Duplicate new patch path");
                    let path = patch_path(value, "b/")?;
                    ensure!(
                        destination.is_none() || destination == path,
                        "Conflicting new patch paths"
                    );
                    destination = path;
                    destination_seen = true;
                } else if let Some(value) = line
                    .strip_prefix("rename from ")
                    .or_else(|| line.strip_prefix("copy from "))
                {
                    let kind = if line.starts_with("rename ") {
                        Kind::Rename
                    } else {
                        Kind::Copy
                    };
                    ensure!(
                        movement.is_none() || movement == Some(kind),
                        "Conflicting rename/copy records"
                    );
                    ensure!(!movement_source, "Duplicate copy/rename source");
                    movement_source = true;
                    movement = Some(kind);
                    let path = decode_path(value)?;
                    ensure!(
                        !source_seen || source.as_ref() == Some(&path),
                        "Conflicting copy/rename source"
                    );
                    source = Some(path);
                } else if let Some(value) = line
                    .strip_prefix("rename to ")
                    .or_else(|| line.strip_prefix("copy to "))
                {
                    let kind = if line.starts_with("rename ") {
                        Kind::Rename
                    } else {
                        Kind::Copy
                    };
                    ensure!(
                        movement.is_none() || movement == Some(kind),
                        "Conflicting rename/copy records"
                    );
                    ensure!(!movement_destination, "Duplicate copy/rename destination");
                    movement_destination = true;
                    movement = Some(kind);
                    let path = decode_path(value)?;
                    ensure!(
                        !destination_seen || destination.as_ref() == Some(&path),
                        "Conflicting copy/rename destination"
                    );
                    destination = Some(path);
                } else if let Some(value) = line.strip_prefix("new file mode ") {
                    ensure!(
                        !created && !deleted && movement.is_none(),
                        "Conflicting file operation"
                    );
                    created = true;
                    new_mode = Some(mode(value)?);
                } else if let Some(value) = line.strip_prefix("deleted file mode ") {
                    ensure!(
                        !created && !deleted && movement.is_none(),
                        "Conflicting file operation"
                    );
                    deleted = true;
                    old_mode = Some(mode(value)?);
                } else if let Some(value) = line.strip_prefix("old mode ") {
                    ensure!(old_mode.is_none(), "Duplicate old mode");
                    old_mode = Some(mode(value)?);
                } else if let Some(value) = line.strip_prefix("new mode ") {
                    ensure!(new_mode.is_none(), "Duplicate new mode");
                    new_mode = Some(mode(value)?);
                } else if let Some(value) = line.strip_prefix("index ") {
                    ensure!(old_oid.is_none(), "Duplicate patch object metadata");
                    let mut parts = value.split_whitespace();
                    let (old, new) = parts
                        .next()
                        .and_then(|part| part.split_once(".."))
                        .context("Invalid patch object metadata")?;
                    for oid in [old, new] {
                        ensure!(
                            (4..=40).contains(&oid.len())
                                && oid.bytes().all(|byte| byte.is_ascii_hexdigit()),
                            "Invalid patch object ID"
                        );
                    }
                    old_oid = Some(old.to_ascii_lowercase());
                    new_oid = Some(new.to_ascii_lowercase());
                    if let Some(value) = parts.next() {
                        let value = mode(value)?;
                        ensure!(
                            old_mode.as_ref().is_none_or(|prior| prior == &value)
                                && new_mode.as_ref().is_none_or(|prior| prior == &value),
                            "Conflicting patch modes"
                        );
                        old_mode.get_or_insert(value.clone());
                        new_mode.get_or_insert(value);
                    }
                    ensure!(parts.next().is_none(), "Unexpected patch object metadata");
                } else if let Some(value) = line
                    .strip_prefix("similarity index ")
                    .or_else(|| line.strip_prefix("dissimilarity index "))
                {
                    let percentage: u8 = value
                        .strip_suffix('%')
                        .context("Invalid patch similarity")?
                        .parse()?;
                    ensure!(percentage <= 100, "Invalid patch similarity");
                } else {
                    break;
                }
                end = position + raw.len();
                i += 1;
            }
            ensure!(
                !(created && source_seen && source.is_some()
                    || deleted && destination_seen && destination.is_some()),
                "New/deleted patch paths are inconsistent"
            );
            let kind = movement.unwrap_or(if created {
                Kind::Add
            } else if deleted {
                Kind::Delete
            } else {
                Kind::Modify
            });
            ensure!(
                new_mode.as_deref() != Some("120000")
                    || (old_mode.as_deref() == Some("120000") && kind == Kind::Modify),
                "Patch introduces a symlink"
            );
            if created {
                ensure!(
                    old_oid
                        .as_ref()
                        .is_none_or(|oid| oid.bytes().all(|byte| byte == b'0')),
                    "New file has a nonempty old object ID"
                );
            }
            if deleted {
                ensure!(
                    new_oid
                        .as_ref()
                        .is_none_or(|oid| oid.bytes().all(|byte| byte == b'0')),
                    "Deleted file has a nonempty new object ID"
                );
            }
            if movement.is_some() {
                ensure!(
                    !created
                        && !deleted
                        && movement_source
                        && movement_destination
                        && source.is_some()
                        && destination.is_some(),
                    "Incomplete copy/rename endpoints"
                );
            } else {
                let name = source
                    .as_ref()
                    .or(destination.as_ref())
                    .cloned()
                    .map(Ok)
                    .unwrap_or_else(|| same_header_path(header))?;
                if kind != Kind::Add {
                    source.get_or_insert(name.clone());
                }
                if kind != Kind::Delete {
                    destination.get_or_insert(name);
                }
            }
            let old = source
                .as_ref()
                .or(destination.as_ref())
                .context("Missing patch path")?;
            let new = destination
                .as_ref()
                .or(source.as_ref())
                .context("Missing patch path")?;
            ensure!(
                header_matches(header, old, new)?,
                "diff --git header contradicts operation paths"
            );
            if kind == Kind::Modify {
                ensure!(
                    old == new,
                    "Path-changing patch needs explicit copy/rename metadata"
                );
            }
            if movement.is_some() {
                ensure!(old != new, "Copy/rename endpoints must differ");
            }
            ensure!(
                hunks || binary.is_some() || kind != Kind::Modify || old_mode != new_mode,
                "Patch block does not describe a supported change"
            );
            operations.push(Operation {
                group,
                source,
                destination,
                kind,
                old_oid,
                new_oid,
                old_mode,
                new_mode,
                range: start..end,
                added,
                removed,
                binary,
            });
        }
        ensure!(
            !operations.is_empty() || text.iter().all(u8::is_ascii_whitespace),
            "Patch contains no supported Git diff"
        );
        for batch in operations.chunk_by(|left, right| left.group == right.group) {
            let mut destinations = BTreeSet::new();
            let mut removals = BTreeSet::new();
            for operation in batch {
                if let Some(path) = &operation.destination {
                    ensure!(destinations.insert(path), "Conflicting patch destinations");
                }
                if matches!(operation.kind, Kind::Delete | Kind::Rename) {
                    ensure!(
                        removals.insert(operation.source.as_ref()),
                        "Conflicting patch removals"
                    );
                }
            }
        }
        Ok(Self { text, operations })
    }

    pub fn paths(&self) -> BTreeSet<String> {
        self.operations.iter().flat_map(Operation::paths).collect()
    }

    pub fn filtered_bytes(&self, selected: &BTreeSet<usize>) -> anyhow::Result<Vec<u8>> {
        let mut output = Vec::new();
        let mut cursor = 0;
        for (index, operation) in self.operations.iter().enumerate() {
            output.extend_from_slice(&self.text[cursor..operation.range.start]);
            if selected.contains(&index) {
                output.extend_from_slice(&self.text[operation.range.clone()]);
            }
            cursor = operation.range.end;
        }
        output.extend_from_slice(&self.text[cursor..]);
        Ok(output)
    }

    pub fn batches(&self, selected: &BTreeSet<usize>) -> Vec<Vec<u8>> {
        let mut batches = Vec::<Vec<u8>>::new();
        let mut group = None;
        for (index, operation) in self.operations.iter().enumerate() {
            if !selected.contains(&index) {
                continue;
            }
            if group != Some(operation.group) {
                batches.push(Vec::new());
                group = Some(operation.group);
            }
            batches
                .last_mut()
                .expect("group initialized")
                .extend_from_slice(&self.text[operation.range.clone()]);
        }
        batches
    }
}

fn strip_eol(raw: &[u8]) -> &[u8] {
    let line = raw.strip_suffix(b"\n").unwrap_or(raw);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn header_line(raw: &[u8]) -> anyhow::Result<&str> {
    std::str::from_utf8(strip_eol(raw)).context("Git patch metadata must be UTF-8")
}

fn mode(value: &str) -> anyhow::Result<String> {
    ensure!(
        matches!(value, "100644" | "100755" | "120000" | "160000"),
        "Unsupported Git patch mode"
    );
    Ok(value.into())
}

fn hunk_counts(value: &str) -> anyhow::Result<(usize, usize)> {
    let fields = value.split_whitespace().take(4).collect::<Vec<_>>();
    ensure!(
        fields.len() == 4 && fields[0] == "@@" && fields[3] == "@@",
        "Malformed patch hunk header"
    );
    fn count(value: &str, prefix: char) -> anyhow::Result<usize> {
        let range = value
            .strip_prefix(prefix)
            .context("Malformed patch hunk range")?;
        let (start, count) = range.split_once(',').unwrap_or((range, "1"));
        let start: usize = start.parse().context("Invalid patch hunk start")?;
        let count: usize = count.parse().context("Invalid patch hunk count")?;
        ensure!(
            start <= MAX_SOURCE_BYTES + 1 && count <= MAX_SOURCE_BYTES + 1,
            "Patch hunk position exceeds the source-processing bound"
        );
        Ok(count)
    }
    Ok((count(fields[1], '-')?, count(fields[2], '+')?))
}

fn patch_path(value: &str, prefix: &str) -> anyhow::Result<Option<String>> {
    let value = value.strip_suffix('\t').unwrap_or(value);
    if value == "/dev/null" {
        return Ok(None);
    }
    let (decoded, count) = unquote(value)?;
    ensure!(count == value.len(), "Trailing data in quoted patch path");
    let path = decoded
        .strip_prefix(prefix)
        .context("Patch path has an unexpected side prefix")?;
    validate_path(path)?;
    Ok(Some(path.into()))
}

fn decode_path(value: &str) -> anyhow::Result<String> {
    let (path, consumed) = unquote(value)?;
    ensure!(consumed == value.len(), "Trailing data in quoted Git path");
    validate_path(&path)?;
    Ok(path)
}

fn validate_path(path: &str) -> anyhow::Result<()> {
    RelativeSafePath::parse(path)?;
    ensure!(
        !path.chars().any(char::is_control),
        "Control characters are not supported in PR paths"
    );
    ensure!(
        !path.contains('\\'),
        "Literal backslash Git filenames are not supported by ADO PR paths"
    );
    Ok(())
}

fn unquote(value: &str) -> anyhow::Result<(String, usize)> {
    if !value.starts_with('"') {
        return Ok((value.into(), value.len()));
    }
    let bytes = value.as_bytes();
    let mut decoded = Vec::new();
    let mut index = 1;
    while index < bytes.len() {
        let byte = bytes[index];
        index += 1;
        if byte == b'"' {
            return Ok((
                String::from_utf8(decoded).context("Git path is not UTF-8")?,
                index,
            ));
        }
        if byte != b'\\' {
            decoded.push(byte);
            continue;
        }
        let escaped = *bytes.get(index).context("Truncated Git path escape")?;
        index += 1;
        decoded.push(match escaped {
            b'\\' | b'"' => escaped,
            b'a' => 7,
            b'b' => 8,
            b't' => 9,
            b'n' => 10,
            b'v' => 11,
            b'f' => 12,
            b'r' => 13,
            b'0'..=b'3' => {
                let next = bytes
                    .get(index..index + 2)
                    .context("Truncated Git octal escape")?;
                ensure!(
                    next.iter().all(|byte| matches!(byte, b'0'..=b'7')),
                    "Invalid Git octal escape"
                );
                index += 2;
                (escaped - b'0') * 64 + (next[0] - b'0') * 8 + (next[1] - b'0')
            }
            _ => anyhow::bail!("Unsupported Git path escape"),
        });
    }
    anyhow::bail!("Unclosed Git path quote")
}

fn header_matches(header: &str, source: &str, destination: &str) -> anyhow::Result<bool> {
    if !source.contains('"')
        && !destination.contains('"')
        && header == format!("a/{source} b/{destination}")
    {
        return Ok(true);
    }
    let (old, consumed) = unquote(header)?;
    if header.starts_with('"') {
        let remainder = header
            .get(consumed..)
            .and_then(|text| text.strip_prefix(' '))
            .context("Missing second Git header path")?;
        let (new, count) = unquote(remainder)?;
        return Ok(old == format!("a/{source}")
            && new == format!("b/{destination}")
            && count == remainder.len());
    }
    if let Some((old, new)) = header.split_once(" \"") {
        let quoted = format!("\"{new}");
        let (new, count) = unquote(&quoted)?;
        return Ok(old == format!("a/{source}")
            && new == format!("b/{destination}")
            && count == quoted.len());
    }
    Ok(false)
}

fn same_header_path(header: &str) -> anyhow::Result<String> {
    if header.starts_with('"') {
        let (path, _) = unquote(header)?;
        let path = path
            .strip_prefix("a/")
            .context("Invalid old Git header path")?;
        validate_path(path)?;
        ensure!(
            header_matches(header, path, path)?,
            "Ambiguous Git header paths"
        );
        return Ok(path.into());
    }
    ensure!(
        header.len() >= 5 && (header.len() - 5).is_multiple_of(2),
        "Ambiguous Git header paths"
    );
    let length = (header.len() - 5) / 2;
    let path = header
        .get(2..2 + length)
        .context("Invalid UTF-8 Git header boundary")?;
    validate_path(path)?;
    ensure!(
        header_matches(header, path, path)?,
        "Ambiguous Git header paths"
    );
    Ok(path.into())
}

fn decode_binary_line(line: &str, output: &mut Vec<u8>) -> anyhow::Result<()> {
    const ALPHABET: &[u8] =
        b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~";
    let bytes = line.as_bytes();
    let count = match bytes.first().copied() {
        Some(byte @ b'A'..=b'Z') => usize::from(byte - b'A') + 1,
        Some(byte @ b'a'..=b'z') => usize::from(byte - b'a') + 27,
        _ => anyhow::bail!("Invalid binary patch line length"),
    };
    ensure!(
        bytes.len() == 1 + count.div_ceil(4) * 5,
        "Malformed binary patch encoding"
    );
    let mut decoded = Vec::new();
    for group in bytes[1..].chunks_exact(5) {
        let mut value = 0u32;
        for byte in group {
            let digit = u32::try_from(
                ALPHABET
                    .iter()
                    .position(|candidate| candidate == byte)
                    .context("Invalid binary patch alphabet")?,
            )?;
            value = value
                .checked_mul(85)
                .and_then(|value| value.checked_add(digit))
                .context("Binary patch encoding overflow")?;
        }
        decoded.extend_from_slice(&value.to_be_bytes());
    }
    output.extend_from_slice(&decoded[..count]);
    Ok(())
}

pub(super) fn delta_size(bytes: &[u8], cursor: &mut usize) -> anyhow::Result<usize> {
    let mut size = 0usize;
    for shift in (0..usize::BITS).step_by(7) {
        let byte = *bytes
            .get(*cursor)
            .context("Truncated binary delta header")?;
        *cursor += 1;
        let part = usize::from(byte & 0x7f);
        ensure!(part <= usize::MAX >> shift, "Binary delta length overflow");
        size |= part << shift;
        if byte & 0x80 == 0 {
            return Ok(size);
        }
    }

    anyhow::bail!("Binary delta length overflow")
}

#[cfg(test)]
mod tests {
    use super::unquote;

    #[test]
    fn quoted_paths_reject_malformed_escapes_without_panicking() {
        for value in [
            "\"unterminated",
            "\"trailing\\",
            "\"bad\\8escape\"",
            "\"bad\\q\"",
            "\"\\0\"",
            "\"\\00\"",
            "\"\\08x\"",
            "\"\\400\"",
            "\"\\377\"",
            "\"\\303\"",
        ] {
            assert!(unquote(value).is_err(), "{value:?}");
        }
    }

    #[test]
    fn quoted_paths_preserve_bytes_and_report_exact_token_boundary() {
        for (value, expected, suffix) in [
            ("\"\\303\\251 guide.md\"\t", "\u{e9} guide.md", "\t"),
            (
                "\"a \\\"quote\\\" and \\\\ slash\" extra",
                "a \"quote\" and \\ slash",
                " extra",
            ),
            ("unquoted file.md", "unquoted file.md", ""),
        ] {
            let (decoded, consumed) = unquote(value).unwrap();
            assert_eq!(decoded, expected);
            assert_eq!(&value[consumed..], suffix);
        }
    }
}
