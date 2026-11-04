//! Look-and-feel settings, as in btop's options: theme background, colour
//! depth, box style, graph symbols and colouring, and the clock.

use serde::{Deserialize, Serialize};

/// A setting with a fixed set of values, which the options screen cycles
/// through and the config file stores by name.
pub trait Choice: Copy + PartialEq + Sized + 'static {
    const ALL: &'static [Self];
    fn name(self) -> &'static str;

    /// The value `delta` steps away, wrapping around.
    fn cycle(self, delta: isize) -> Self {
        let i = Self::ALL.iter().position(|&c| c == self).unwrap_or(0) as isize;
        Self::ALL[(i + delta).rem_euclid(Self::ALL.len() as isize) as usize]
    }
}

macro_rules! choice {
    ($(#[$m:meta])* $name:ident { $($(#[$vm:meta])* $variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
        #[serde(rename_all = "kebab-case")]
        pub enum $name {
            $($(#[$vm])* $variant),+
        }

        impl Choice for $name {
            const ALL: &'static [Self] = &[$($name::$variant),+];
            fn name(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }
        }
    };
}

choice! {
    /// Colour depth: detect from the terminal, or force one.
    TrueColor {
        #[default]
        Auto => "auto",
        On => "on",
        Off => "off",
    }
}

choice! {
    /// Panel border drawing.
    BoxStyle {
        #[default]
        Rounded => "rounded",
        Plain => "plain",
        Double => "double",
        Thick => "thick",
    }
}

choice! {
    /// Characters used to draw graphs: braille dots (finest), block
    /// eighths, or shade characters that work on a bare TTY.
    GraphSymbol {
        Braille => "braille",
        #[default]
        Block => "block",
        Tty => "tty",
    }
}

choice! {
    /// Graph colouring: the tool's colour, or the theme's low → high
    /// gradient by height.
    GraphColor {
        #[default]
        Accent => "accent",
        Gradient => "gradient",
    }
}

choice! {
    /// What the per-tool chart plots.
    ChartMetric {
        #[default]
        Cost => "cost",
        Tokens => "tokens",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices() {
        assert_eq!(GraphSymbol::Block.cycle(1), GraphSymbol::Tty);
        assert_eq!(GraphSymbol::Braille.cycle(-1), GraphSymbol::Tty);
        assert_eq!(BoxStyle::default(), BoxStyle::Rounded);
        assert_eq!(TrueColor::On.name(), "on");
        let v: GraphSymbol =
            toml::from_str::<std::collections::HashMap<String, GraphSymbol>>("g = \"braille\"").unwrap()["g"];
        assert_eq!(v, GraphSymbol::Braille);
    }
}
