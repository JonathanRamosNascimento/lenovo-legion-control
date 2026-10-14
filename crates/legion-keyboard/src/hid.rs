//! Minimal HID report descriptor parser.
//!
//! Legion Control never sends a report to a device unless the descriptor the
//! device itself publishes (`/sys/class/hidraw/*/device/report_descriptor`)
//! declares exactly the report layout the code expects. This parser extracts,
//! for every Input/Output/Feature main item, which usage lives at which
//! position of which report, and inside which collections.
//!
//! Only what is needed for that check is implemented (short items, push/pop,
//! usage ranges); long items are skipped as the HID specification requires.

use std::fmt;

/// Longest descriptor the kernel accepts (`HID_MAX_DESCRIPTOR_SIZE`).
pub const MAX_DESCRIPTOR_LEN: usize = 4096;
/// Upper bound for expanded usage ranges (guards against hostile input).
const MAX_ELEMENTS: u32 = 4096;

/// Kind of report a field belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    /// Device → host data.
    Input,
    /// Host → device data on the interrupt/control pipe.
    Output,
    /// Bidirectional configuration data (`GET_REPORT` / `SET_REPORT`).
    Feature,
}

/// A run of identical data elements of one report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// Report kind.
    pub kind: ReportKind,
    /// Report id (`0` when the device does not use numbered reports).
    pub report_id: u8,
    /// `(usage page, usage)` of the top-level collection.
    pub application: (u16, u16),
    /// `(usage page, usage)` of the innermost collection.
    pub collection: (u16, u16),
    /// Usage page of the element.
    pub usage_page: u16,
    /// Usage of the element (`0` for padding / unlabelled bits).
    pub usage: u16,
    /// Size of one element in bits.
    pub bits: u32,
    /// Number of consecutive elements with this usage and size.
    pub count: u32,
}

/// Why a descriptor could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The descriptor ends in the middle of an item.
    Truncated,
    /// Larger than the kernel accepts.
    TooLong,
    /// More collections closed than opened.
    UnbalancedCollection,
    /// A usage range or report count that is not plausible.
    TooManyElements,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ParseError::Truncated => "truncated report descriptor",
            ParseError::TooLong => "report descriptor too long",
            ParseError::UnbalancedCollection => "unbalanced collections in report descriptor",
            ParseError::TooManyElements => "implausible report size in report descriptor",
        })
    }
}

impl std::error::Error for ParseError {}

/// A parsed report descriptor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Descriptor {
    /// Every data field, in descriptor order.
    pub fields: Vec<Field>,
}

#[derive(Clone, Default)]
struct Globals {
    usage_page: u32,
    report_size: u32,
    report_count: u32,
    report_id: u8,
}

#[derive(Default)]
struct Locals {
    usages: Vec<u32>,
    min: Option<u32>,
    max: Option<u32>,
}

struct Open {
    page: u16,
    usage: u16,
}

fn le(data: &[u8]) -> u32 {
    data.iter()
        .enumerate()
        .fold(0u32, |v, (i, b)| v | (u32::from(*b) << (8 * i)))
}

/// Splits a (possibly 32-bit) usage into `(page, usage)`.
fn split_usage(raw: u32, size: usize, page: u32) -> (u16, u16) {
    if size == 4 {
        ((raw >> 16) as u16, raw as u16)
    } else {
        (page as u16, raw as u16)
    }
}

impl Descriptor {
    /// Parses a raw report descriptor.
    pub fn parse(raw: &[u8]) -> Result<Self, ParseError> {
        if raw.len() > MAX_DESCRIPTOR_LEN {
            return Err(ParseError::TooLong);
        }
        let mut fields = Vec::new();
        let mut global = Globals::default();
        let mut stack: Vec<Globals> = Vec::new();
        let mut local = Locals::default();
        let mut collections: Vec<Open> = Vec::new();
        // usages are stored with their own page when the item was 32-bit
        let mut pages: Vec<u16> = Vec::new();

        let mut i = 0;
        while i < raw.len() {
            let prefix = raw[i];
            i += 1;
            if prefix == 0xFE {
                // long item: bDataSize, bLongItemTag, data
                let len = *raw.get(i).ok_or(ParseError::Truncated)? as usize;
                i = i
                    .checked_add(2 + len)
                    .filter(|e| *e <= raw.len())
                    .ok_or(ParseError::Truncated)?;
                continue;
            }
            let size = match prefix & 0b11 {
                3 => 4,
                n => n as usize,
            };
            let kind = (prefix >> 2) & 0b11;
            let tag = prefix >> 4;
            let data = raw.get(i..i + size).ok_or(ParseError::Truncated)?;
            i += size;
            let value = le(data);

            match kind {
                // Main
                0 => {
                    match tag {
                        0x8..=0x9 | 0xB => {
                            let rk = match tag {
                                0x8 => ReportKind::Input,
                                0x9 => ReportKind::Output,
                                _ => ReportKind::Feature,
                            };
                            let constant = value & 1 == 1;
                            emit(
                                &mut fields,
                                rk,
                                constant,
                                &global,
                                &local,
                                &pages,
                                &collections,
                            )?;
                        }
                        0xA => {
                            let (page, usage) = match local.usages.first() {
                                Some(u) => (
                                    pages.first().copied().unwrap_or(global.usage_page as u16),
                                    *u as u16,
                                ),
                                None => (global.usage_page as u16, 0),
                            };
                            collections.push(Open { page, usage });
                        }
                        0xC => {
                            collections.pop().ok_or(ParseError::UnbalancedCollection)?;
                        }
                        _ => {}
                    }
                    local = Locals::default();
                    pages.clear();
                }
                // Global
                1 => match tag {
                    0x0 => global.usage_page = value,
                    0x7 => global.report_size = value,
                    0x8 => global.report_id = value as u8,
                    0x9 => global.report_count = value,
                    0xA => stack.push(global.clone()),
                    0xB => global = stack.pop().ok_or(ParseError::UnbalancedCollection)?,
                    _ => {}
                },
                // Local
                2 => match tag {
                    0x0 => {
                        let (page, usage) = split_usage(value, size, global.usage_page);
                        local.usages.push(u32::from(usage));
                        pages.push(page);
                    }
                    0x1 => local.min = Some(value),
                    0x2 => local.max = Some(value),
                    _ => {}
                },
                _ => {}
            }
        }
        if !collections.is_empty() {
            return Err(ParseError::UnbalancedCollection);
        }
        Ok(Self { fields })
    }

    /// Fields of one report.
    pub fn report(&self, kind: ReportKind, report_id: u8) -> impl Iterator<Item = &Field> {
        self.fields
            .iter()
            .filter(move |f| f.kind == kind && f.report_id == report_id)
    }

    /// Size in bytes of a report **without** its report id byte.
    pub fn report_bytes(&self, kind: ReportKind, report_id: u8) -> usize {
        let bits: u64 = self
            .report(kind, report_id)
            .map(|f| u64::from(f.bits) * u64::from(f.count))
            .sum();
        bits.div_ceil(8) as usize
    }

    /// Distinct `(usage page, usage)` of the top-level collections.
    pub fn applications(&self) -> Vec<(u16, u16)> {
        let mut v: Vec<(u16, u16)> = Vec::new();
        for f in &self.fields {
            if !v.contains(&f.application) {
                v.push(f.application);
            }
        }
        v
    }
}

fn emit(
    out: &mut Vec<Field>,
    kind: ReportKind,
    constant: bool,
    g: &Globals,
    l: &Locals,
    pages: &[u16],
    collections: &[Open],
) -> Result<(), ParseError> {
    if g.report_count > MAX_ELEMENTS || g.report_size > 256 {
        return Err(ParseError::TooManyElements);
    }
    let (Some(app), Some(inner)) = (collections.first(), collections.last()) else {
        // data outside any collection: not valid HID, ignore
        return Ok(());
    };
    // expand the usage list (explicit usages, or a min..=max range)
    let mut usages: Vec<(u16, u16)> = l
        .usages
        .iter()
        .zip(pages)
        .map(|(u, p)| (*p, *u as u16))
        .collect();
    if usages.is_empty() {
        if let (Some(min), Some(max)) = (l.min, l.max) {
            if max < min || max - min >= MAX_ELEMENTS {
                return Err(ParseError::TooManyElements);
            }
            usages.extend((min..=max).map(|u| (g.usage_page as u16, u as u16)));
        }
    }
    let mut runs: Vec<Field> = Vec::new();
    for n in 0..g.report_count as usize {
        // The "constant" bit alone does not make a field padding: read-only
        // fields (e.g. LampArray attributes) are constant *and* labelled.
        let (usage_page, usage) = if constant && usages.is_empty() {
            (0, 0)
        } else {
            usages
                .get(n)
                .or_else(|| usages.last())
                .copied()
                .unwrap_or((g.usage_page as u16, 0))
        };
        match runs.last_mut() {
            Some(last) if last.usage_page == usage_page && last.usage == usage => last.count += 1,
            _ => runs.push(Field {
                kind,
                report_id: g.report_id,
                application: (app.page, app.usage),
                collection: (inner.page, inner.usage),
                usage_page,
                usage,
                bits: g.report_size,
                count: 1,
            }),
        }
    }
    out.extend(runs);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_boot_keyboard_subset() {
        // Usage Page (Generic Desktop), Usage (Keyboard), Collection (Application),
        //   Report ID 1, Report Size 8, Report Count 2, Usage (X), Usage (Y),
        //   Feature (Data,Var,Abs), End Collection
        let d = Descriptor::parse(&[
            0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, 0x01, 0x75, 0x08, 0x95, 0x02, 0x09, 0x30,
            0x09, 0x31, 0xB1, 0x02, 0xC0,
        ])
        .unwrap();
        assert_eq!(d.applications(), [(0x01, 0x06)]);
        assert_eq!(d.fields.len(), 2);
        assert_eq!(d.fields[0].usage, 0x30);
        assert_eq!(d.fields[1].usage, 0x31);
        assert_eq!(d.report_bytes(ReportKind::Feature, 1), 2);
    }

    #[test]
    fn same_usage_elements_are_collapsed() {
        // 8 × 16-bit elements sharing one usage
        let d = Descriptor::parse(&[
            0x06, 0x59, 0x00, 0x09, 0x01, 0xA1, 0x01, 0x85, 0x04, 0x09, 0x21, 0x75, 0x10, 0x95,
            0x08, 0xB1, 0x02, 0xC0,
        ])
        .unwrap();
        assert_eq!(d.fields.len(), 1);
        assert_eq!((d.fields[0].bits, d.fields[0].count), (16, 8));
        assert_eq!(d.report_bytes(ReportKind::Feature, 4), 16);
    }

    #[test]
    fn usage_ranges_and_constants() {
        // Usage Min/Max 1..=3 (three 1-bit fields), then 5 bits of padding
        let d = Descriptor::parse(&[
            0x05, 0x08, 0x09, 0x01, 0xA1, 0x01, 0x19, 0x01, 0x29, 0x03, 0x75, 0x01, 0x95, 0x03,
            0x91, 0x02, 0x95, 0x01, 0x75, 0x05, 0x91, 0x03, 0xC0,
        ])
        .unwrap();
        let usages: Vec<u16> = d.fields.iter().map(|f| f.usage).collect();
        assert_eq!(usages, [1, 2, 3, 0]);
        assert_eq!(d.report_bytes(ReportKind::Output, 0), 1);
    }

    #[test]
    fn rejects_malformed_input() {
        assert_eq!(
            Descriptor::parse(&[0x06, 0x59]).unwrap_err(),
            ParseError::Truncated
        );
        assert_eq!(
            Descriptor::parse(&[0xC0]).unwrap_err(),
            ParseError::UnbalancedCollection
        );
        assert_eq!(
            Descriptor::parse(&[0x05, 0x01, 0x09, 0x06, 0xA1, 0x01]).unwrap_err(),
            ParseError::UnbalancedCollection
        );
        assert_eq!(
            Descriptor::parse(&vec![0u8; MAX_DESCRIPTOR_LEN + 1]).unwrap_err(),
            ParseError::TooLong
        );
        // report count far beyond anything plausible
        assert_eq!(
            Descriptor::parse(&[
                0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x75, 0x08, 0x96, 0xFF, 0xFF, 0x09, 0x30, 0xB1,
                0x02, 0xC0
            ])
            .unwrap_err(),
            ParseError::TooManyElements
        );
    }

    #[test]
    fn long_items_are_skipped() {
        let d = Descriptor::parse(&[
            0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0xFE, 0x02, 0x10, 0xAA, 0xBB, 0x75, 0x08, 0x95,
            0x01, 0x09, 0x30, 0xB1, 0x02, 0xC0,
        ])
        .unwrap();
        assert_eq!(d.fields.len(), 1);
    }
}
