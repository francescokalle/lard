//! sRGB colors with hex (de)serialization and cheap animation helpers.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt;

/// A straight-alpha sRGB color.
///
/// Config files, themes and UI code speak sRGB. Conversion to the
/// premultiplied linear space expected by the GPU happens exactly once, at the
/// renderer boundary (see [`Color::premultiplied_linear`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Self = Self::rgba(0.0, 0.0, 0.0, 0.0);
    pub const BLACK: Self = Self::rgb(0.0, 0.0, 0.0);
    pub const WHITE: Self = Self::rgb(1.0, 1.0, 1.0);

    #[must_use]
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    #[must_use]
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    #[must_use]
    pub const fn from_rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: a as f32 / 255.0,
        }
    }

    #[must_use]
    pub const fn from_rgb8(r: u8, g: u8, b: u8) -> Self {
        Self::from_rgba8(r, g, b, 255)
    }

    /// Parse `#RGB`, `#RGBA`, `#RRGGBB` or `#RRGGBBAA`.
    ///
    /// Input is treated as bytes so a non-ASCII string is a clean error, never
    /// a panic on a UTF-8 slice boundary.
    pub fn from_hex(input: &str) -> Result<Self, String> {
        let s = input.trim();
        let bytes = s.strip_prefix('#').unwrap_or(s).as_bytes();
        let nibble = |b: u8| -> Result<u8, String> {
            match b {
                b'0'..=b'9' => Ok(b - b'0'),
                b'a'..=b'f' => Ok(b - b'a' + 10),
                b'A'..=b'F' => Ok(b - b'A' + 10),
                _ => Err(format!("invalid hex color {input:?}")),
            }
        };
        let byte =
            |i: usize| -> Result<u8, String> { Ok(nibble(bytes[i])? * 16 + nibble(bytes[i + 1])?) };
        match bytes.len() {
            3 | 4 => {
                let r = nibble(bytes[0])? * 17;
                let g = nibble(bytes[1])? * 17;
                let b = nibble(bytes[2])? * 17;
                let a = if bytes.len() == 4 {
                    nibble(bytes[3])? * 17
                } else {
                    255
                };
                Ok(Self::from_rgba8(r, g, b, a))
            }
            6 => Ok(Self::from_rgba8(byte(0)?, byte(2)?, byte(4)?, 255)),
            8 => Ok(Self::from_rgba8(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => Err(format!(
                "invalid hex color {input:?}: expected #RGB, #RGBA, #RRGGBB or #RRGGBBAA"
            )),
        }
    }

    #[must_use]
    pub fn to_hex(self) -> String {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        if self.a >= 1.0 {
            format!("#{:02X}{:02X}{:02X}", q(self.r), q(self.g), q(self.b))
        } else {
            format!(
                "#{:02X}{:02X}{:02X}{:02X}",
                q(self.r),
                q(self.g),
                q(self.b),
                q(self.a)
            )
        }
    }

    #[must_use]
    pub const fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    #[must_use]
    pub fn scale_alpha(self, factor: f32) -> Self {
        Self {
            a: (self.a * factor).clamp(0.0, 1.0),
            ..self
        }
    }

    #[must_use]
    pub fn mix(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        Self {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
            a: self.a + (other.a - self.a) * t,
        }
    }

    /// Linear RGB with straight alpha. The shader premultiplies after mixing
    /// fill and border, which keeps borders visually correct.
    #[must_use]
    pub fn linear_straight(self) -> [f32; 4] {
        [
            srgb_to_linear(self.r.clamp(0.0, 1.0)),
            srgb_to_linear(self.g.clamp(0.0, 1.0)),
            srgb_to_linear(self.b.clamp(0.0, 1.0)),
            self.a.clamp(0.0, 1.0),
        ]
    }

    /// Premultiplied linear RGBA, ready for the GPU.
    ///
    /// The surface format is `*Srgb`, so shaders operate in linear space. This
    /// is the only conversion performed per draw and it is branch-free.
    #[must_use]
    pub fn premultiplied_linear(self) -> [f32; 4] {
        let a = self.a.clamp(0.0, 1.0);
        let r = srgb_to_linear(self.r.clamp(0.0, 1.0)) * a;
        let g = srgb_to_linear(self.g.clamp(0.0, 1.0)) * a;
        let b = srgb_to_linear(self.b.clamp(0.0, 1.0)) * a;
        [r, g, b, a]
    }
}

/// IEC 61966-2-1 sRGB transfer function.
#[inline]
#[must_use]
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::TRANSPARENT
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_hex(&s).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_hex_forms() {
        assert_eq!(Color::from_hex("#FFF").unwrap(), Color::WHITE);
        assert_eq!(Color::from_hex("#00000000").unwrap(), Color::TRANSPARENT);
        assert_eq!(
            Color::from_hex("#8FB8FF").unwrap(),
            Color::from_rgb8(0x8f, 0xb8, 0xff)
        );
        assert_eq!(
            Color::from_hex("8FB8FF80").unwrap(),
            Color::from_rgba8(0x8f, 0xb8, 0xff, 0x80)
        );
    }

    #[test]
    fn non_ascii_is_an_error_not_a_panic() {
        assert!(Color::from_hex("aé€").is_err());
        assert!(Color::from_hex("#γ").is_err());
    }

    #[test]
    fn roundtrips() {
        let c = Color::from_hex("#123456CC").unwrap();
        assert_eq!(c.to_hex(), "#123456CC");
    }

    #[test]
    fn transparent_is_zero_premultiplied() {
        assert_eq!(Color::TRANSPARENT.premultiplied_linear(), [0.0; 4]);
    }
}
