//! Reads the page geometry out of a markup's `ExtraAnnotationData`, a Qt
//! `QDataStream`-serialized `QVariantMap` (big-endian, UTF-16 keys).
//!
//! Verified on a Libra Colour (firmware 4.45, DbVersion 176): 19 keys,
//! including `MarkupRect` (the ink's bounding box) and `RangeRect` (the text
//! the markup is anchored to), both `QRect`s in page pixels.

/// A rectangle in page pixels, as Qt stores a `QRect`: inclusive edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MarkupGeometry {
    /// Page size in pixels (`ScreenDimensions`).
    pub page: Option<(i32, i32)>,
    /// Bounding box of the ink.
    pub ink: Option<Rect>,
    /// The text the markup is anchored to.
    pub range: Option<Rect>,
}

impl MarkupGeometry {
    /// Reads what it can. Parsing stops quietly at anything unexpected, so
    /// a firmware change costs the crop, never the import.
    pub fn parse(data: &[u8]) -> Self {
        let mut geometry = Self::default();
        let mut r = Reader { data, at: 0 };
        let Some(count) = r.u32() else {
            return geometry;
        };
        for _ in 0..count {
            let Some(key) = r.string() else { break };
            let Some(value) = r.variant() else { break };
            match (key.as_str(), value) {
                ("ScreenDimensions", Value::Size(w, h)) => geometry.page = Some((w, h)),
                ("MarkupRect", Value::Rect(rect)) => geometry.ink = Some(rect),
                ("RangeRect", Value::Rect(rect)) => geometry.range = Some(rect),
                _ => {}
            }
        }
        geometry
    }

    /// The part of the page worth showing: full width, from the top of the
    /// ink or its text to the bottom of either, with some margin.
    pub fn crop(&self) -> Option<Rect> {
        const MARGIN: i32 = 40;
        let (width, height) = self.page?;
        let rects = [self.ink, self.range];
        let mut rects = rects.iter().flatten();
        let first = rects.next()?;
        let (top, bottom) = rects.fold((first.top, first.bottom), |(t, b), r| {
            (t.min(r.top), b.max(r.bottom))
        });
        let top = (top - MARGIN).max(0);
        let bottom = (bottom + MARGIN).min(height - 1);
        (top < bottom).then_some(Rect {
            left: 0,
            top,
            right: width - 1,
            bottom,
        })
    }
}

enum Value {
    Rect(Rect),
    Size(i32, i32),
    Other,
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let bytes = self.data.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(bytes)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }

    fn i32(&mut self) -> Option<i32> {
        Some(self.u32()? as i32)
    }

    /// A `QString`: byte length (0xFFFFFFFF for null), then UTF-16BE.
    fn string(&mut self) -> Option<String> {
        let len = self.u32()?;
        if len == u32::MAX {
            return Some(String::new());
        }
        let units: Vec<u16> = self
            .take(len as usize)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_be_bytes(c))
            .collect();
        Some(String::from_utf16_lossy(&units))
    }

    /// A `QVariant`: type ID, null flag, then the value. Only the types seen
    /// in markups are understood; anything else ends parsing.
    fn variant(&mut self) -> Option<Value> {
        let kind = self.u32()?;
        self.take(1)?; // is-null flag
        Some(match kind {
            1 => {
                self.take(1)?; // bool
                Value::Other
            }
            2 | 3 => {
                self.take(4)?; // int, uint
                Value::Other
            }
            6 => {
                self.take(8)?; // double
                Value::Other
            }
            10 => {
                self.string()?;
                Value::Other
            }
            19 => Value::Rect(Rect {
                left: self.i32()?,
                top: self.i32()?,
                right: self.i32()?,
                bottom: self.i32()?,
            }),
            21 => Value::Size(self.i32()?, self.i32()?),
            // A user type: its name, then its data. `Fingerprint` is a QVector<int>.
            127 => {
                let name_len = self.u32()? as usize;
                if self.take(name_len)? != b"QVector<int>\0" {
                    return None;
                }
                let count = self.u32()? as usize;
                self.take(count.checked_mul(4)?)?;
                Value::Other
            }
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(out: &mut Vec<u8>, name: &str) {
        let units: Vec<u8> = name.encode_utf16().flat_map(u16::to_be_bytes).collect();
        out.extend((units.len() as u32).to_be_bytes());
        out.extend(units);
    }

    fn ints(out: &mut Vec<u8>, kind: u32, values: &[i32]) {
        out.extend(kind.to_be_bytes());
        out.push(0);
        for v in values {
            out.extend(v.to_be_bytes());
        }
    }

    /// Shaped like a real Libra Colour markup (trimmed to a few keys).
    fn sample() -> Vec<u8> {
        let mut out = 6u32.to_be_bytes().to_vec();
        key(&mut out, "StartKey");
        ints(&mut out, 2, &[15169]);
        key(&mut out, "ScreenDimensions");
        ints(&mut out, 21, &[1264, 1680]);
        key(&mut out, "ReadingFontFamily");
        out.extend(10u32.to_be_bytes());
        out.push(0);
        key(&mut out, "KF Sourcerer");
        key(&mut out, "RangeRect");
        ints(&mut out, 19, &[60, 900, 1151, 1093]);
        key(&mut out, "Fingerprint");
        out.extend(127u32.to_be_bytes());
        out.push(0);
        out.extend(13u32.to_be_bytes());
        out.extend(b"QVector<int>\0");
        out.extend(2u32.to_be_bytes());
        out.extend(27i32.to_be_bytes());
        out.extend(26i32.to_be_bytes());
        key(&mut out, "MarkupRect");
        ints(&mut out, 19, &[58, 1066, 1235, 1221]);
        out
    }

    #[test]
    fn reads_markup_geometry() {
        let g = MarkupGeometry::parse(&sample());
        assert_eq!(g.page, Some((1264, 1680)));
        assert_eq!(
            g.ink,
            Some(Rect {
                left: 58,
                top: 1066,
                right: 1235,
                bottom: 1221
            })
        );
        assert_eq!(g.range.map(|r| r.top), Some(900));
        assert_eq!(
            g.crop(),
            Some(Rect {
                left: 0,
                top: 860,
                right: 1263,
                bottom: 1261
            })
        );
    }

    #[test]
    fn stops_quietly_on_unknown_or_truncated_data() {
        let data = sample();
        // Cut inside MarkupRect: everything before it is still read.
        let g = MarkupGeometry::parse(&data[..data.len() - 6]);
        assert_eq!(g.range.map(|r| r.top), Some(900));
        assert_eq!(g.ink, None);
        assert_eq!(MarkupGeometry::parse(&[]), MarkupGeometry::default());
        assert_eq!(MarkupGeometry::default().crop(), None);
    }
}
