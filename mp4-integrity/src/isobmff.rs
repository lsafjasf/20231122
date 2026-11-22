//! Minimal ISO-BMFF (MP4/QuickTime) box walker.
//!
//! The walker records every box it can find, including unknown/private
//! boxes, together with byte offsets, track ownership and a repair verdict.

use std::fmt;
use std::fs;
use std::path::Path;

/// Four-character-code identifying a box type.
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct FourCc(pub [u8; 4]);

impl FourCc {
    pub fn from_bytes(b: &[u8]) -> Self {
        let mut v = [b'?'; 4];
        v.copy_from_slice(&b[..4]);
        FourCc(v)
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap_or("????")
    }

    fn is_printable(&self) -> bool {
        self.0.iter().all(|b| (0x20..=0x7e).contains(b))
    }
}

impl fmt::Display for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_printable() {
            write!(f, "{}", self.as_str())
        } else {
            write!(
                f,
                "0x{:02x}{:02x}{:02x}{:02x}",
                self.0[0], self.0[1], self.0[2], self.0[3]
            )
        }
    }
}

impl fmt::Debug for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FourCc({})", self)
    }
}

/// What is wrong with a box, if anything.
#[allow(dead_code)] // Malformed is part of the public diagnostic vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    /// Type code is not one of the standard boxes at this nesting level.
    UnknownType,
    /// Declared size extends past the end of the file (or parent box).
    Truncated,
    /// Declared size is too small to hold the box header.
    InvalidSize,
    /// Unrecognised extended-size / uuid framing.
    Malformed,
}

/// Whether and how a box can be handled by the repair pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Repairability {
    /// Box is structurally fine; nothing to do.
    Ok,
    /// Box can be dropped while keeping the rest of the container playable.
    Droppable,
    /// Box is required (e.g. moov/trak); dropping it destroys media metadata.
    Required,
    /// Truncation can be trimmed away so the remaining boxes stay valid.
    TrimTail,
}

impl Repairability {
    pub fn label(self) -> &'static str {
        match self {
            Repairability::Ok => "ok",
            Repairability::Droppable => "droppable",
            Repairability::Required => "required (not repairable in place)",
            Repairability::TrimTail => "trim-tail",
        }
    }
}

/// One box found while walking the file.
#[allow(dead_code)] // header_size/available_size/is_ok/body_offset are public API.
#[derive(Clone, Debug)]
pub struct BoxNode {
    pub box_type: FourCc,
    /// Absolute byte offset of the 32-bit size field.
    pub offset: u64,
    /// Header size (8 or 16 bytes, more for full/extended boxes).
    pub header_size: u64,
    /// Size claimed by the box header (including the header itself).
    pub declared_size: u64,
    /// Bytes actually available for this box in the file.
    pub available_size: u64,
    /// Nesting depth (top-level boxes are depth 0).
    pub depth: usize,
    /// Track id from the enclosing `trak`, if any (0 = file-level).
    pub track_id: u32,
    /// Human readable track owner, e.g. "track 2 (video)".
    pub owner: String,
    pub error: Option<ErrorKind>,
    pub repair: Repairability,
    /// True for standard boxes this tool understands, false for private ones.
    pub known: bool,
}

impl BoxNode {
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }

    pub fn body_offset(&self) -> u64 {
        self.offset + self.header_size
    }
}

/// A structural problem located during the walk.
#[allow(dead_code)] // kind is exposed for programmatic consumers.
#[derive(Clone, Debug)]
pub struct StructuralError {
    pub offset: u64,
    pub box_type: Option<FourCc>,
    pub kind: ErrorKind,
    pub message: String,
}

/// Full result of inspecting one container file.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub file_size: u64,
    pub boxes: Vec<BoxNode>,
    pub errors: Vec<StructuralError>,
    /// Track ids discovered via `tkhd` boxes.
    pub track_ids: Vec<u32>,
    /// Media kind label per track id, read from `hdlr` (video/audio/...).
    pub track_kinds: std::collections::BTreeMap<u32, String>,
}

impl Report {
    /// Offset of the first structural error, if any.
    pub fn first_error_offset(&self) -> Option<u64> {
        self.errors.iter().map(|e| e.offset).min()
    }

    pub fn error_boxes(&self) -> impl Iterator<Item = &BoxNode> {
        self.boxes.iter().filter(|b| b.error.is_some())
    }

    pub fn unknown_boxes(&self) -> impl Iterator<Item = &BoxNode> {
        self.boxes.iter().filter(|b| !b.known)
    }

    pub fn is_healthy(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Top-level ISO-BMFF / QuickTime box types defined by the specs.
const KNOWN_TOP_LEVEL: &[&str] = &[
    "ftyp", "moov", "mdat", "free", "skip", "wide", "pdin", "styp", "sidx", "ssix", "prft", "meco",
    "mere", "uuid", "stz2",
];

/// Boxes that contain child boxes instead of raw payload.
const CONTAINER_BOXES: &[&str] = &[
    "moov", "trak", "mdia", "minf", "stbl", "udta", "edts", "dinf", "mvex", "moof", "traf", "mfra",
    "meco", "mere", "ipro", "sinf", "schi", "strk", "strd", "stri", "sttg",
];

/// Boxes whose payload starts with one version byte + three flag bytes.
#[allow(dead_code)] // documents the full-box framing table for future use.
const FULL_BOXES: &[&str] = &[
    "mvhd", "tkhd", "mdhd", "hdlr", "elst", "stsd", "stts", "ctts", "stsc", "stsz", "stz2", "stco",
    "co64", "stss", "cslg", "sthd", "dref", "vmhd", "smhd", "nmhd", "sdtp", "sbgp", "sgpd", "stsh",
    "padb",
];

/// Boxes that are mandatory for a playable movie (only relevant at top level).
const REQUIRED_TOP_LEVEL: &[&str] = &["ftyp", "moov"];

fn known_top_level(cc: FourCc) -> bool {
    KNOWN_TOP_LEVEL.contains(&cc.as_str())
}

fn is_container(cc: FourCc) -> bool {
    CONTAINER_BOXES.contains(&cc.as_str())
}

/// Boxes recognised anywhere in the tree (top-level or nested).
fn is_known_anywhere(cc: FourCc) -> bool {
    const KNOWN_NESTED: &[&str] = &[
        "mvhd", "trak", "tkhd", "mdia", "mdhd", "hdlr", "minf", "vmhd", "smhd", "nmhd", "sthd",
        "dinf", "dref", "url ", "urn ", "stbl", "stsd", "stts", "ctts", "stsc", "stsz", "stz2",
        "stco", "co64", "stss", "stsh", "cslg", "udta", "meta", "ilst", "edts", "elst", "mvex",
        "mehd", "trex", "moof", "mfhd", "traf", "tfhd", "tfdt", "trun", "sdtp", "sbgp", "sgpd",
        "subt", "saiz", "saio", "avc1", "avc3", "hev1", "hvc1", "mp4a", "esds", "avcC", "hvcC",
        "btrt", "pasp", "colr", "clap", "chan", "wave", "frma", "schm", "sinf", "schi", "styp",
        "sidx", "prft", "meco", "mere", "stz2", "sthd", "stvi", "stri", "strk", "strd", "sttg",
        "stsg", "stpp", "wvtt",
    ];
    KNOWN_TOP_LEVEL.contains(&cc.as_str())
        || CONTAINER_BOXES.contains(&cc.as_str())
        || KNOWN_NESTED.contains(&cc.as_str())
}

/// Read the whole file and walk its box tree.
pub fn inspect_file(path: &Path) -> anyhow::Result<Report> {
    let data = fs::read(path)?;
    Ok(inspect_bytes(&data))
}

/// Walk an in-memory ISO-BMFF image.
pub fn inspect_bytes(data: &[u8]) -> Report {
    let mut report = Report {
        file_size: data.len() as u64,
        ..Report::default()
    };
    let mut ctx = WalkCtx {
        data,
        report: &mut report,
    };
    ctx.walk_siblings(0, data.len() as u64, 0, 0, String::from("file"));
    ctx.collect_track_ids();
    ctx.assign_owners();
    report
}

/// Parse the track id out of a `tkhd` full box.
fn read_tkhd_id(data: &[u8], offset: u64, declared_size: u64) -> Option<u32> {
    // tkhd: 8-byte box header, then version(1)+flags(3), then per-version dates.
    let p = offset + 8 + 4;
    if p + 4 > offset + declared_size || p + 4 > data.len() as u64 {
        return None;
    }
    let version = data[p as usize];
    let id_pos = if version == 1 {
        p + 1 + 8 + 8
    } else {
        p + 1 + 4 + 4
    };
    if id_pos + 4 > offset + declared_size || id_pos + 4 > data.len() as u64 {
        return None;
    }
    Some(u32::from_be_bytes([
        data[id_pos as usize],
        data[(id_pos + 1) as usize],
        data[(id_pos + 2) as usize],
        data[(id_pos + 3) as usize],
    ]))
}

struct WalkCtx<'a> {
    data: &'a [u8],
    report: &'a mut Report,
}

impl<'a> WalkCtx<'a> {
    fn assign_owners(&mut self) {
        // Track ids first, then handler kinds, then human-readable owners.
        self.collect_handler_kinds();
        // Rebuild track ownership from trak intervals so truncated containers
        // (where hdlr may be missing) still attribute nested boxes correctly.
        let traks: Vec<(u64, u64, u32)> = self
            .report
            .boxes
            .iter()
            .filter(|b| b.box_type.as_str() == "trak")
            .map(|b| (b.offset, b.offset + b.declared_size, b.track_id))
            .collect();
        for node in self.report.boxes.iter_mut() {
            if node.depth == 0 {
                if node.owner.is_empty() {
                    node.owner = "file".to_string();
                }
                continue;
            }
            if let Some((_, _, track_id)) = traks
                .iter()
                .find(|(start, end, _)| node.offset >= *start && node.offset < *end)
            {
                node.track_id = *track_id;
                let kind = self
                    .report
                    .track_kinds
                    .get(track_id)
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                node.owner = format!("track {} ({})", track_id, kind);
            }
        }
    }

    /// Scan `hdlr` boxes and remember the handler_type for each trak.
    fn collect_handler_kinds(&mut self) {
        let data = self.data;
        let trak_ranges: Vec<(u64, u64, u32)> = self
            .report
            .boxes
            .iter()
            .filter(|b| b.box_type.as_str() == "trak")
            .map(|b| (b.offset, b.offset + b.declared_size, b.track_id))
            .collect();
        // Snapshot (offset, size, track_id) of hdlr boxes.
        let hdlrs: Vec<(u64, u64)> = self
            .report
            .boxes
            .iter()
            .filter(|b| b.box_type.as_str() == "hdlr")
            .map(|b| (b.offset, b.declared_size))
            .collect();
        for (offset, size) in hdlrs {
            // hdlr: 8 header + 4 version/flags + 4 pre_defined + 4 handler_type
            let pos = offset + 8 + 4 + 4;
            if pos + 4 > offset + size || pos + 4 > data.len() as u64 {
                continue;
            }
            let Some((_, _, track_id)) = trak_ranges
                .iter()
                .find(|(start, end, _)| offset >= *start && offset < *end)
            else {
                continue;
            };
            let handler = &data[pos as usize..pos as usize + 4];
            let label = match handler {
                b"vide" => "video",
                b"soun" => "audio",
                b"hint" => "hint",
                b"meta" => "metadata",
                b"text" | b"subt" => "subtitle",
                _ => "other",
            };
            self.report.track_kinds.insert(*track_id, label.to_string());
        }
    }

    fn walk_siblings(&mut self, start: u64, end: u64, depth: usize, track_id: u32, owner: String) {
        let mut cursor = start;
        while cursor < end {
            let node = self.read_box(cursor, end, depth, track_id, owner.clone());
            let (node, new_errors) = match node {
                Ok(pair) => pair,
                Err(err) => {
                    self.report.errors.push(err);
                    return;
                }
            };
            self.report.errors.extend(new_errors);
            let next = node.offset + node.declared_size;
            let box_type = node.box_type;
            self.report.boxes.push(node);

            if box_type.as_str() == "trak" {
                self.walk_children(cursor, next, end, depth + 1, 0, String::new());
            } else if is_container(box_type) {
                self.walk_children(cursor, next, end, depth + 1, track_id, owner.clone());
            }
            cursor = next;
        }
    }

    /// Find every `tkhd` box in the current box list and map its enclosing
    /// trak offset to the declared track id. Works even when the trak itself
    /// is truncated, as long as the tkhd header survived.
    fn collect_track_ids(&mut self) {
        let data = self.data;
        let tkhds: Vec<(u64, u32)> = self
            .report
            .boxes
            .iter()
            .filter(|b| b.box_type.as_str() == "tkhd")
            .filter_map(|b| read_tkhd_id(data, b.offset, b.declared_size).map(|id| (b.offset, id)))
            .collect();
        let trak_ranges: Vec<(u64, u64)> = self
            .report
            .boxes
            .iter()
            .filter(|b| b.box_type.as_str() == "trak")
            .map(|b| (b.offset, b.offset + b.declared_size))
            .collect();
        for (tkhd_offset, id) in tkhds {
            if let Some((start, _end)) = trak_ranges
                .iter()
                .find(|(start, end)| tkhd_offset >= *start && tkhd_offset < *end)
            {
                for node in self
                    .report
                    .boxes
                    .iter_mut()
                    .filter(|b| b.box_type.as_str() == "trak" && b.offset == *start)
                {
                    node.track_id = id;
                }
                if !self.report.track_ids.contains(&id) {
                    self.report.track_ids.push(id);
                }
            }
        }
    }

    fn walk_children(
        &mut self,
        parent_start: u64,
        parent_next: u64,
        hard_end: u64,
        depth: usize,
        track_id: u32,
        owner: String,
    ) {
        // Locate the parent header to find where its payload starts.
        let parent_header = self.header_size_at(parent_start, hard_end);
        let body_start = match parent_header {
            Ok(header) => parent_start + header,
            Err(_) => return,
        };
        let body_end = parent_next.min(hard_end);
        if body_start >= body_end {
            return;
        }
        self.walk_siblings(body_start, body_end, depth, track_id, owner);
    }

    fn header_size_at(&self, offset: u64, hard_end: u64) -> Result<u64, StructuralError> {
        let data = self.data;
        if offset + 8 > hard_end {
            return Err(StructuralError {
                offset,
                box_type: None,
                kind: ErrorKind::Truncated,
                message: "box header extends past end of file".into(),
            });
        }
        let size32 = u32::from_be_bytes([
            data[offset as usize],
            data[(offset + 1) as usize],
            data[(offset + 2) as usize],
            data[(offset + 3) as usize],
        ]);
        if size32 == 1 {
            if offset + 16 > hard_end {
                return Err(StructuralError {
                    offset,
                    box_type: None,
                    kind: ErrorKind::Truncated,
                    message: "extended-size box header is truncated".into(),
                });
            }
            Ok(16)
        } else {
            Ok(8)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn read_box(
        &self,
        offset: u64,
        region_end: u64,
        depth: usize,
        track_id: u32,
        owner: String,
    ) -> Result<(BoxNode, Vec<StructuralError>), StructuralError> {
        let data = self.data;
        let mut found_errors = Vec::new();
        if offset + 8 > region_end {
            return Err(StructuralError {
                offset,
                box_type: None,
                kind: ErrorKind::Truncated,
                message: "fewer than 8 bytes remain; box header is truncated".into(),
            });
        }
        let size32 = u32::from_be_bytes([
            data[offset as usize],
            data[(offset + 1) as usize],
            data[(offset + 2) as usize],
            data[(offset + 3) as usize],
        ]);
        let box_type = FourCc::from_bytes(&data[(offset + 4) as usize..(offset + 8) as usize]);

        let (header_size, declared_size) = match size32 {
            1 => {
                if offset + 16 > region_end {
                    return Err(StructuralError {
                        offset,
                        box_type: Some(box_type),
                        kind: ErrorKind::Truncated,
                        message: format!(
                            "box {} declares extended size but 64-bit field is missing",
                            box_type
                        ),
                    });
                }
                let ext = u64::from_be_bytes(
                    data[(offset + 8) as usize..(offset + 16) as usize]
                        .try_into()
                        .unwrap(),
                );
                (16u64, ext)
            }
            0 => {
                // Box extends to the end of the region (usually end of file).
                (8u64, region_end.saturating_sub(offset))
            }
            n => (8u64, u64::from(n)),
        };

        let mut error = None;
        if declared_size < header_size {
            error = Some(ErrorKind::InvalidSize);
            found_errors.push(StructuralError {
                offset,
                box_type: Some(box_type),
                kind: ErrorKind::InvalidSize,
                message: format!(
                    "box {} declares size {} which is smaller than its {}-byte header",
                    box_type, declared_size, header_size
                ),
            });
        }

        let available_size = region_end.saturating_sub(offset);
        if error.is_none() && offset + declared_size > region_end {
            error = Some(ErrorKind::Truncated);
            found_errors.push(StructuralError {
                offset,
                box_type: Some(box_type),
                kind: ErrorKind::Truncated,
                message: format!(
                    "box {} at offset {} declares {} bytes but only {} bytes remain",
                    box_type, offset, declared_size, available_size
                ),
            });
        }

        // Unknown-type detection is meaningful at file level; inside a
        // container we only flag boxes we have never heard of.
        let known = if depth == 0 {
            known_top_level(box_type)
        } else {
            is_known_anywhere(box_type)
        };
        if !known && error.is_none() {
            error = Some(ErrorKind::UnknownType);
            found_errors.push(StructuralError {
                offset,
                box_type: Some(box_type),
                kind: ErrorKind::UnknownType,
                message: format!(
                    "unknown/private box {} at offset {} (not part of the standard layout)",
                    box_type, offset
                ),
            });
        }

        let repair = if error.is_none() {
            Repairability::Ok
        } else if error == Some(ErrorKind::UnknownType) {
            // Unknown top-level/private boxes can be skipped; unknown boxes
            // inside a trak are left alone (marked required-ish).
            if depth == 0 {
                Repairability::Droppable
            } else {
                Repairability::Required
            }
        } else if error == Some(ErrorKind::Truncated) && depth == 0 {
            if REQUIRED_TOP_LEVEL.contains(&box_type.as_str()) {
                Repairability::Required
            } else {
                Repairability::TrimTail
            }
        } else {
            Repairability::Required
        };

        Ok((
            BoxNode {
                box_type,
                offset,
                header_size,
                declared_size,
                available_size,
                depth,
                track_id,
                owner,
                error,
                repair,
                known,
            },
            found_errors,
        ))
    }
}
