//! Colour themes. Each theme maps a small palette onto egui visuals.
//!
//! The built-in themes are constants; the ones the user makes (Settings → Appearance and
//! Terminal, saved as `config::CustomTheme` / `config::CustomTermTheme`) are registered
//! with `set_custom` and found by name like the others.
use crate::config::{CustomTermTheme, CustomTheme};
use crate::style::icons::IconPack;
use egui::{Color32, Stroke, Visuals};
use std::sync::{LazyLock, Mutex, RwLock};

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Theme {
    pub name: &'static str,
    pub dark: bool,
    pub bg: Color32,
    pub panel: Color32,
    pub surface: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub accent: Color32,
    pub selection: Color32,
    pub marked: Color32,
    pub dir: Color32,
    pub archive: Color32,
    pub border: Color32,
    pub danger: Color32,
    /// syntect theme used for code previews.
    pub syntax: &'static str,
    /// Widget corner radius (windows and cards scale from it).
    pub radius: u8,
    /// Icon style used when the icon setting is "Match theme".
    pub icons: IconPack,
    /// The 16 ANSI colours of a terminal in this theme (`None`: a general light or dark set).
    pub ansi: Option<&'static [u32; 16]>,
}

const fn hex(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

// ANSI colours: black, red, green, yellow, blue, magenta, cyan, white, then the bright ones.
const BREEZE_LIGHT: [u32; 16] = [
    0x232629, 0xda4453, 0x1b8a4c, 0xb35a00, 0x1b7fcf, 0x8e44ad, 0x16857a, 0x707d8a, //
    0x4d5459, 0xc0392b, 0x27ae60, 0xd46a00, 0x3daee9, 0x9b59b6, 0x1abc9c, 0xa1a9b1,
];
const BREEZE_DARK: [u32; 16] = [
    0x31363b, 0xed1515, 0x11d116, 0xf67400, 0x1d99f3, 0x9b59b6, 0x1abc9c, 0xfcfcfc, //
    0x7f8c8d, 0xc0392b, 0x1cdc9a, 0xfdbc4b, 0x3daee9, 0x8e44ad, 0x16a085, 0xffffff,
];
const GITHUB_LIGHT: [u32; 16] = [
    0x24292f, 0xcf222e, 0x116329, 0x9a6700, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781, //
    0x57606a, 0xa40e26, 0x1a7f37, 0x7d4e00, 0x218bff, 0xa475f9, 0x3192aa, 0x8c959f,
];
const GITHUB_DARK: [u32; 16] = [
    0x484f58, 0xff7b72, 0x3fb950, 0xd29922, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xb1bac4, //
    0x6e7681, 0xffa198, 0x56d364, 0xe3b341, 0x79c0ff, 0xd2a8ff, 0x56d4dd, 0xf0f6fc,
];
const ONE_LIGHT: [u32; 16] = [
    0x383a42, 0xe45649, 0x50a14f, 0xc18401, 0x4078f2, 0xa626a4, 0x0184bc, 0xa0a1a7, //
    0x696c77, 0xe45649, 0x50a14f, 0xc18401, 0x4078f2, 0xa626a4, 0x0184bc, 0xd4d4d6,
];
const ONE_DARK: [u32; 16] = [
    0x3f4451, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, //
    0x5c6370, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff,
];
const CATPPUCCIN_LATTE: [u32; 16] = [
    0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be, //
    0x6c6f85, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xbcc0cc,
];
const CATPPUCCIN_MOCHA: [u32; 16] = [
    0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, //
    0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
];
const SOLARIZED: [u32; 16] = [
    0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5, //
    0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
];
const GRUVBOX_LIGHT: [u32; 16] = [
    0x7c6f64, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984, //
    0x928374, 0x9d0006, 0x79740e, 0xb57614, 0x076678, 0x8f3f71, 0x427b58, 0x3c3836,
];
const GRUVBOX_DARK: [u32; 16] = [
    0x3c3836, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984, //
    0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
];
const DRACULA: [u32; 16] = [
    0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2, //
    0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
];
const NORD: [u32; 16] = [
    0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0, //
    0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
];
const TOKYO_NIGHT: [u32; 16] = [
    0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, //
    0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
];
const MONOKAI: [u32; 16] = [
    0x3e3d32, 0xf92672, 0xa6e22e, 0xf4bf75, 0x66d9ef, 0xae81ff, 0xa1efe4, 0xf8f8f2, //
    0x75715e, 0xf92672, 0xa6e22e, 0xf4bf75, 0x66d9ef, 0xae81ff, 0xa1efe4, 0xf9f8f5,
];
const ROSE_PINE: [u32; 16] = [
    0x26233a, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4, //
    0x6e6a86, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
];
const KANAGAWA: [u32; 16] = [
    0x363646, 0xc34043, 0x76946a, 0xc0a36e, 0x7e9cd8, 0x957fb8, 0x6a9589, 0xc8c093, //
    0x727169, 0xe82424, 0x98bb6c, 0xe6c384, 0x7fb4ca, 0x938aa9, 0x7aa89f, 0xdcd7ba,
];
const EVERFOREST: [u32; 16] = [
    0x475258, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xd3c6aa, //
    0x7a8478, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xf2efdf,
];
const GREEN_PHOSPHOR: [u32; 16] = [
    0x1a2b1a, 0xff6b5b, 0x00d75f, 0xc8d65f, 0x3fb98f, 0x8fd18f, 0x5fd7af, 0xb8e6b8, //
    0x3d5c3d, 0xff8a7a, 0x5fff87, 0xdfff87, 0x5fd7af, 0xafffaf, 0x87ffd7, 0xe0ffe0,
];
const AMBER_PHOSPHOR: [u32; 16] = [
    0x2b1f0a, 0xff6b4a, 0xd7a600, 0xffc24a, 0xc98a2b, 0xe0a060, 0xd9b36c, 0xf2d6a2, //
    0x5c4520, 0xff8a6a, 0xf0c000, 0xffd77a, 0xe0a84a, 0xf0b880, 0xeccb8c, 0xffecc8,
];

const CATPPUCCIN_FRAPPE: [u32; 16] = [
    0x51576d, 0xe78284, 0xa6d189, 0xe5c890, 0x8caaee, 0xf4b8e4, 0x81c8be, 0xb5bfe2, //
    0x626880, 0xe78284, 0xa6d189, 0xe5c890, 0x8caaee, 0xf4b8e4, 0x81c8be, 0xa5adce,
];
const CATPPUCCIN_MACCHIATO: [u32; 16] = [
    0x494d64, 0xed8796, 0xa6da95, 0xeed49f, 0x8aadf4, 0xf5bde6, 0x8bd5ca, 0xb8c0e0, //
    0x5b6078, 0xed8796, 0xa6da95, 0xeed49f, 0x8aadf4, 0xf5bde6, 0x8bd5ca, 0xa5adcb,
];
const ROSE_PINE_MOON: [u32; 16] = [
    0x393552, 0xeb6f92, 0x3e8fb0, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xea9a97, 0xe0def4, //
    0x6e6a86, 0xeb6f92, 0x3e8fb0, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xea9a97, 0xe0def4,
];
const ROSE_PINE_DAWN: [u32; 16] = [
    0xf2e9e1, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279, //
    0x9893a5, 0xb4637a, 0x286983, 0xea9d34, 0x56949f, 0x907aa9, 0xd7827e, 0x575279,
];
const KANAGAWA_DRAGON: [u32; 16] = [
    0x0d0c0c, 0xc4746e, 0x8a9a7b, 0xc4b28a, 0x8ba4b0, 0xa292a3, 0x8ea4a2, 0xc8c093, //
    0xa6a69c, 0xe46876, 0x87a987, 0xe6c384, 0x7fb4ca, 0x938aa9, 0x7aa89f, 0xc5c9c5,
];
const EVERFOREST_LIGHT: [u32; 16] = [
    0x5c6a72, 0xf85552, 0x8da101, 0xdfa000, 0x3a94c5, 0xdf69ba, 0x35a77c, 0xa6b0a0, //
    0x829181, 0xf85552, 0x8da101, 0xdfa000, 0x3a94c5, 0xdf69ba, 0x35a77c, 0xe0dcc7,
];
const TOKYO_NIGHT_LIGHT: [u32; 16] = [
    0x0f0f14, 0x8c4351, 0x485e30, 0x8f5e15, 0x34548a, 0x5a4a78, 0x0f4b6e, 0x828594, //
    0x6c6e75, 0x8c4351, 0x485e30, 0x8f5e15, 0x34548a, 0x5a4a78, 0x0f4b6e, 0x9699a3,
];
const AYU_LIGHT: [u32; 16] = [
    0x000000, 0xea6c6d, 0x6cbf43, 0xeca944, 0x3199e1, 0x9e75c7, 0x46ba94, 0xbababa, //
    0x686868, 0xf07171, 0x86b300, 0xf2ae49, 0x399ee6, 0xa37acc, 0x4cbf99, 0xd1d1d1,
];
const AYU_MIRAGE: [u32; 16] = [
    0x191e2a, 0xed8274, 0xa6cc70, 0xfad07b, 0x6dcbfa, 0xcfbafa, 0x90e1c6, 0xc7c7c7, //
    0x686868, 0xf28779, 0xbae67e, 0xffd580, 0x73d0ff, 0xd4bfff, 0x95e6cb, 0xffffff,
];
const AYU_DARK: [u32; 16] = [
    0x11151c, 0xea6c73, 0x7fd962, 0xf9af4f, 0x53bdfa, 0xcda1fa, 0x90e1c6, 0xc7c7c7, //
    0x686868, 0xf07178, 0xaad94c, 0xffb454, 0x59c2ff, 0xd2a6ff, 0x95e6cb, 0xffffff,
];
const NORD_LIGHT: [u32; 16] = [
    0x3b4252, 0xbf616a, 0x4f7a43, 0x9a7a2c, 0x5e81ac, 0x8f6a8a, 0x3f8a9a, 0xd8dee9, //
    0x4c566a, 0xbf616a, 0x5f8a4e, 0xb08a3a, 0x81a1c1, 0xb48ead, 0x5a9ea8, 0xe5e9f0,
];
const MATERIAL: [u32; 16] = [
    0x546e7a, 0xff5370, 0xc3e88d, 0xffcb6b, 0x82aaff, 0xc792ea, 0x89ddff, 0xeeffff, //
    0x546e7a, 0xff5370, 0xc3e88d, 0xffcb6b, 0x82aaff, 0xc792ea, 0x89ddff, 0xffffff,
];
const MATERIAL_OCEAN: [u32; 16] = [
    0x3b3f51, 0xf07178, 0xc3e88d, 0xffcb6b, 0x82aaff, 0xc792ea, 0x89ddff, 0xa6accd, //
    0x464b5d, 0xff5370, 0xc3e88d, 0xffcb6b, 0x82aaff, 0xc792ea, 0x89ddff, 0xffffff,
];

const fn mix_channel(a: u32, b: u32, shift: u32, percent: u32) -> u8 {
    let (a, b) = ((a >> shift) & 0xff, (b >> shift) & 0xff);
    ((a * (100 - percent) + b * percent) / 100) as u8
}

const fn mix(a: u32, b: u32, percent: u32) -> Color32 {
    Color32::from_rgb(mix_channel(a, b, 16, percent), mix_channel(a, b, 8, percent), mix_channel(a, b, 0, percent))
}

/// A terminal colour scheme. Its window's info bar takes its colours from it too.
const fn scheme(name: &'static str, dark: bool, bg: u32, fg: u32, accent: u32, ansi: &'static [u32; 16]) -> Theme {
    Theme {
        name,
        dark,
        bg: hex(bg),
        panel: mix(bg, fg, 4),
        surface: mix(bg, fg, 10),
        text: hex(fg),
        weak: mix(fg, bg, 35),
        accent: hex(accent),
        selection: mix(bg, accent, 25),
        marked: hex(ansi[3]),
        dir: hex(ansi[4]),
        archive: hex(ansi[5]),
        border: mix(bg, fg, 18),
        danger: hex(ansi[1]),
        syntax: "InspiredGitHub",
        radius: 5,
        icons: IconPack::Matrix,
        ansi: Some(ansi),
    }
}

/// An app theme from its palette, in the order of `Theme`'s fields (bg, panel, surface,
/// text, weak, accent, selection, marked, dir, archive, border, danger).
const fn app(name: &'static str, dark: bool, c: [u32; 12], syntax: &'static str, ansi: &'static [u32; 16]) -> Theme {
    Theme {
        name,
        dark,
        bg: hex(c[0]),
        panel: hex(c[1]),
        surface: hex(c[2]),
        text: hex(c[3]),
        weak: hex(c[4]),
        accent: hex(c[5]),
        selection: hex(c[6]),
        marked: hex(c[7]),
        dir: hex(c[8]),
        archive: hex(c[9]),
        border: hex(c[10]),
        danger: hex(c[11]),
        syntax,
        radius: 5,
        icons: IconPack::Matrix,
        ansi: Some(ansi),
    }
}

/// The colour schemes offered for terminal profiles.
pub const TERM_THEMES: &[Theme] = &[
    scheme("Breeze Light", false, 0xffffff, 0x232629, 0x3daee9, &BREEZE_LIGHT),
    scheme("Breeze Dark", true, 0x1b1e20, 0xfcfcfc, 0x3daee9, &BREEZE_DARK),
    scheme("GitHub Light", false, 0xffffff, 0x24292f, 0x0969da, &GITHUB_LIGHT),
    scheme("GitHub Dark", true, 0x0d1117, 0xc9d1d9, 0x58a6ff, &GITHUB_DARK),
    scheme("One Light", false, 0xfafafa, 0x383a42, 0x526fff, &ONE_LIGHT),
    scheme("One Dark", true, 0x282c34, 0xabb2bf, 0x528bff, &ONE_DARK),
    scheme("Catppuccin Latte", false, 0xeff1f5, 0x4c4f69, 0x8839ef, &CATPPUCCIN_LATTE),
    scheme("Catppuccin Frappé", true, 0x303446, 0xc6d0f5, 0xca9ee6, &CATPPUCCIN_FRAPPE),
    scheme("Catppuccin Macchiato", true, 0x24273a, 0xcad3f5, 0xc6a0f6, &CATPPUCCIN_MACCHIATO),
    scheme("Catppuccin Mocha", true, 0x1e1e2e, 0xcdd6f4, 0xcba6f7, &CATPPUCCIN_MOCHA),
    scheme("Solarized Light", false, 0xfdf6e3, 0x657b83, 0x268bd2, &SOLARIZED),
    scheme("Solarized Dark", true, 0x002b36, 0x839496, 0x2aa198, &SOLARIZED),
    scheme("Gruvbox Light", false, 0xfbf1c7, 0x3c3836, 0xaf3a03, &GRUVBOX_LIGHT),
    scheme("Gruvbox Dark", true, 0x282828, 0xebdbb2, 0xfe8019, &GRUVBOX_DARK),
    scheme("Dracula", true, 0x282a36, 0xf8f8f2, 0xbd93f9, &DRACULA),
    scheme("Nord", true, 0x2e3440, 0xd8dee9, 0x88c0d0, &NORD),
    scheme("Tokyo Night", true, 0x1a1b26, 0xc0caf5, 0x7aa2f7, &TOKYO_NIGHT),
    scheme("Tokyo Night Light", false, 0xd5d6db, 0x343b58, 0x34548a, &TOKYO_NIGHT_LIGHT),
    scheme("Monokai", true, 0x272822, 0xf8f8f2, 0xa6e22e, &MONOKAI),
    scheme("Rosé Pine", true, 0x191724, 0xe0def4, 0xc4a7e7, &ROSE_PINE),
    scheme("Rosé Pine Moon", true, 0x232136, 0xe0def4, 0xc4a7e7, &ROSE_PINE_MOON),
    scheme("Rosé Pine Dawn", false, 0xfaf4ed, 0x575279, 0x907aa9, &ROSE_PINE_DAWN),
    scheme("Kanagawa", true, 0x1f1f28, 0xdcd7ba, 0x7e9cd8, &KANAGAWA),
    scheme("Kanagawa Dragon", true, 0x181616, 0xc5c9c5, 0x8ba4b0, &KANAGAWA_DRAGON),
    scheme("Everforest", true, 0x2d353b, 0xd3c6aa, 0xa7c080, &EVERFOREST),
    scheme("Everforest Light", false, 0xfdf6e3, 0x5c6a72, 0x8da101, &EVERFOREST_LIGHT),
    scheme("Ayu Light", false, 0xfcfcfc, 0x5c6166, 0xffaa33, &AYU_LIGHT),
    scheme("Ayu Mirage", true, 0x1f2430, 0xcccac2, 0xffcc66, &AYU_MIRAGE),
    scheme("Ayu Dark", true, 0x0b0e14, 0xbfbdb6, 0xe6b450, &AYU_DARK),
    scheme("Nord Light", false, 0xeceff4, 0x2e3440, 0x5e81ac, &NORD_LIGHT),
    scheme("Material", true, 0x263238, 0xeeffff, 0x80cbc4, &MATERIAL),
    scheme("Material Ocean", true, 0x0f111a, 0xa6accd, 0x84ffff, &MATERIAL_OCEAN),
    scheme("Green Phosphor", true, 0x0a120a, 0x7dff9c, 0x00d75f, &GREEN_PHOSPHOR),
    scheme("Amber Phosphor", true, 0x140e04, 0xffc24a, 0xffa200, &AMBER_PHOSPHOR),
];

/// A terminal profile's colour theme: a scheme, or (from older settings) an app theme.
pub fn term_by_name(name: &str) -> &'static Theme {
    TERM_THEMES
        .iter()
        .find(|t| t.name == name)
        .or_else(|| CUSTOM.read().ok().and_then(|c| c.1.iter().copied().find(|t| t.name == name)))
        .unwrap_or_else(|| by_name(name))
}

pub const THEMES: &[Theme] = &[
    // ------------------------------------------------------------ native OS looks
    Theme {
        name: "macOS Light",
        dark: false,
        bg: hex(0xffffff),
        panel: hex(0xf2f2f4),
        surface: hex(0xe6e6ea),
        text: hex(0x1d1d1f),
        weak: hex(0x86868b),
        accent: hex(0x007aff),
        selection: hex(0xd6e6fd),
        marked: hex(0x9e5d08),
        dir: hex(0x3b9cf0),
        archive: hex(0x8e8e93),
        border: hex(0xe1e1e6),
        danger: hex(0xff3b30),
        syntax: "InspiredGitHub",
        radius: 6,
        icons: IconPack::MacOS,
        ansi: None,
    },
    Theme {
        name: "macOS Dark",
        dark: true,
        bg: hex(0x1e1e1e),
        panel: hex(0x2a2a2c),
        surface: hex(0x3a3a3c),
        text: hex(0xf5f5f7),
        weak: hex(0x98989d),
        accent: hex(0x0a84ff),
        selection: hex(0x1d3b63),
        marked: hex(0xff9f0a),
        dir: hex(0x64b5f6),
        archive: hex(0xa1a1a6),
        border: hex(0x3a3a3c),
        danger: hex(0xff453a),
        syntax: "base16-ocean.dark",
        radius: 6,
        icons: IconPack::MacOS,
        ansi: None,
    },
    Theme {
        name: "Adwaita Light",
        dark: false,
        bg: hex(0xffffff),
        panel: hex(0xebebed),
        surface: hex(0xdedee2),
        text: hex(0x2e2e33),
        weak: hex(0x77767b),
        accent: hex(0x3584e4),
        selection: hex(0xd3e3f8),
        marked: hex(0xc04400),
        dir: hex(0x3584e4),
        archive: hex(0x9141ac),
        border: hex(0xdededf),
        danger: hex(0xe01b24),
        syntax: "InspiredGitHub",
        radius: 8,
        icons: IconPack::Adwaita,
        ansi: None,
    },
    Theme {
        name: "Adwaita Dark",
        dark: true,
        bg: hex(0x1d1d20),
        panel: hex(0x2e2e32),
        surface: hex(0x3a3a3f),
        text: hex(0xf6f5f4),
        weak: hex(0x9a9996),
        accent: hex(0x78aeed),
        selection: hex(0x26405e),
        marked: hex(0xffa348),
        dir: hex(0x78aeed),
        archive: hex(0xc061cb),
        border: hex(0x38383d),
        danger: hex(0xff7b63),
        syntax: "base16-ocean.dark",
        radius: 8,
        icons: IconPack::Adwaita,
        ansi: None,
    },
    Theme {
        name: "Windows 11 Light",
        dark: false,
        bg: hex(0xfbfbfb),
        panel: hex(0xf3f3f3),
        surface: hex(0xe9e9e9),
        text: hex(0x1a1a1a),
        weak: hex(0x5f5f5f),
        accent: hex(0x0067c0),
        selection: hex(0xcce4f7),
        marked: hex(0xab5300),
        dir: hex(0xe8a317),
        archive: hex(0x8764b8),
        border: hex(0xe5e5e5),
        danger: hex(0xc42b1c),
        syntax: "InspiredGitHub",
        radius: 4,
        icons: IconPack::Windows,
        ansi: None,
    },
    Theme {
        name: "Windows 11 Dark",
        dark: true,
        bg: hex(0x1c1c1c),
        panel: hex(0x202020),
        surface: hex(0x2d2d2d),
        text: hex(0xffffff),
        weak: hex(0x9d9d9d),
        accent: hex(0x4cc2ff),
        selection: hex(0x1f3d55),
        marked: hex(0xfce100),
        dir: hex(0xffc83d),
        archive: hex(0xb4a0ff),
        border: hex(0x333333),
        danger: hex(0xff99a4),
        syntax: "base16-ocean.dark",
        radius: 4,
        icons: IconPack::Windows,
        ansi: None,
    },
    Theme {
        name: "Breeze Light",
        dark: false,
        bg: hex(0xffffff),
        panel: hex(0xeff0f1),
        surface: hex(0xe0e1e3),
        text: hex(0x232629),
        weak: hex(0x707d8a),
        accent: hex(0x3daee9),
        selection: hex(0xcde9f8),
        marked: hex(0xaf5200),
        dir: hex(0x3daee9),
        archive: hex(0x9b59b6),
        border: hex(0xd1d2d3),
        danger: hex(0xda4453),
        syntax: "InspiredGitHub",
        radius: 3,
        icons: IconPack::Breeze,
        ansi: Some(&BREEZE_LIGHT),
    },
    Theme {
        name: "Breeze Dark",
        dark: true,
        bg: hex(0x1b1e20),
        panel: hex(0x2a2e32),
        surface: hex(0x31363b),
        text: hex(0xfcfcfc),
        weak: hex(0xa1a9b1),
        accent: hex(0x3daee9),
        selection: hex(0x1e4258),
        marked: hex(0xf67400),
        dir: hex(0x3daee9),
        archive: hex(0x9b59b6),
        border: hex(0x3b4045),
        danger: hex(0xda4453),
        syntax: "base16-ocean.dark",
        radius: 3,
        icons: IconPack::Breeze,
        ansi: Some(&BREEZE_DARK),
    },
    Theme {
        name: "Digital Rain",
        dark: true,
        bg: hex(0x030806),
        panel: hex(0x07110b),
        surface: hex(0x0d1f14),
        text: hex(0xb8f5cb),
        weak: hex(0x4f8a60),
        accent: hex(0x39ff8a),
        selection: hex(0x0f3a21),
        marked: hex(0xe8ff6a),
        dir: hex(0x5cf2a0),
        archive: hex(0x9dffcf),
        border: hex(0x123020),
        danger: hex(0xff5c6c),
        syntax: "base16-ocean.dark",
        radius: 5,
        icons: IconPack::Matrix,
        ansi: None,
    },
    Theme {
        name: "Classic Commander",
        dark: true,
        bg: hex(0x000080),
        panel: hex(0x0000a8),
        surface: hex(0x00006e),
        text: hex(0x55ffff),
        weak: hex(0xa8a8a8),
        accent: hex(0xffff55),
        selection: hex(0x00a8a8),
        marked: hex(0xffff55),
        dir: hex(0xffffff),
        archive: hex(0xff55ff),
        border: hex(0x55ffff),
        danger: hex(0xff5555),
        syntax: "base16-ocean.dark",
        radius: 5,
        icons: IconPack::Matrix,
        ansi: None,
    },
    Theme {
        name: "Dracula",
        dark: true,
        bg: hex(0x282a36),
        panel: hex(0x21222c),
        surface: hex(0x343746),
        text: hex(0xf8f8f2),
        weak: hex(0x8b93b8),
        accent: hex(0xbd93f9),
        selection: hex(0x44475a),
        marked: hex(0xf1fa8c),
        dir: hex(0x8be9fd),
        archive: hex(0xff79c6),
        border: hex(0x44475a),
        danger: hex(0xff5555),
        syntax: "base16-mocha.dark",
        radius: 5,
        icons: IconPack::Matrix,
        ansi: Some(&DRACULA),
    },
    Theme {
        name: "Nord",
        dark: true,
        bg: hex(0x2e3440),
        panel: hex(0x3b4252),
        surface: hex(0x434c5e),
        text: hex(0xeceff4),
        weak: hex(0x9aa5b8),
        accent: hex(0x88c0d0),
        selection: hex(0x4c566a),
        marked: hex(0xebcb8b),
        dir: hex(0x81a1c1),
        archive: hex(0xb48ead),
        border: hex(0x4c566a),
        danger: hex(0xbf616a),
        syntax: "base16-ocean.dark",
        radius: 5,
        icons: IconPack::Matrix,
        ansi: Some(&NORD),
    },
    Theme {
        name: "Solarized Dark",
        dark: true,
        bg: hex(0x002b36),
        panel: hex(0x073642),
        surface: hex(0x0a4250),
        text: hex(0x93a1a1),
        weak: hex(0x657b83),
        accent: hex(0xb58900),
        selection: hex(0x174652),
        marked: hex(0xee8040),
        dir: hex(0x268bd2),
        archive: hex(0xd33682),
        border: hex(0x0f4b59),
        danger: hex(0xdc322f),
        syntax: "Solarized (dark)",
        radius: 5,
        icons: IconPack::Matrix,
        ansi: Some(&SOLARIZED),
    },
    Theme {
        name: "Solarized Light",
        dark: false,
        bg: hex(0xfdf6e3),
        panel: hex(0xeee8d5),
        surface: hex(0xfffbef),
        text: hex(0x586e75),
        weak: hex(0x818e8e),
        accent: hex(0xb58900),
        selection: hex(0xe3dcc6),
        marked: hex(0xb54314),
        dir: hex(0x268bd2),
        archive: hex(0xd33682),
        border: hex(0xd8d0b8),
        danger: hex(0xdc322f),
        syntax: "Solarized (light)",
        radius: 5,
        icons: IconPack::Matrix,
        ansi: Some(&SOLARIZED),
    },
    Theme {
        name: "Gruvbox Dark",
        dark: true,
        bg: hex(0x282828),
        panel: hex(0x32302f),
        surface: hex(0x3c3836),
        text: hex(0xebdbb2),
        weak: hex(0xa89984),
        accent: hex(0xfe8019),
        selection: hex(0x504945),
        marked: hex(0xfabd2f),
        dir: hex(0x83a598),
        archive: hex(0xd3869b),
        border: hex(0x504945),
        danger: hex(0xfb4934),
        syntax: "base16-eighties.dark",
        radius: 5,
        icons: IconPack::Matrix,
        ansi: Some(&GRUVBOX_DARK),
    },
    Theme {
        name: "Monokai",
        dark: true,
        bg: hex(0x272822),
        panel: hex(0x1e1f1c),
        surface: hex(0x3e3d32),
        text: hex(0xf8f8f2),
        weak: hex(0x90908a),
        accent: hex(0xa6e22e),
        selection: hex(0x49483e),
        marked: hex(0xe6db74),
        dir: hex(0x66d9ef),
        archive: hex(0xae81ff),
        border: hex(0x49483e),
        danger: hex(0xf92672),
        syntax: "base16-mocha.dark",
        radius: 5,
        icons: IconPack::Matrix,
        ansi: Some(&MONOKAI),
    },
    Theme {
        name: "Catppuccin Mocha",
        dark: true,
        bg: hex(0x1e1e2e),
        panel: hex(0x181825),
        surface: hex(0x313244),
        text: hex(0xcdd6f4),
        weak: hex(0x9399b2),
        accent: hex(0xcba6f7),
        selection: hex(0x45475a),
        marked: hex(0xf9e2af),
        dir: hex(0x89b4fa),
        archive: hex(0xf5c2e7),
        border: hex(0x45475a),
        danger: hex(0xf38ba8),
        syntax: "base16-ocean.dark",
        radius: 5,
        icons: IconPack::Matrix,
        ansi: Some(&CATPPUCCIN_MOCHA),
    },
    Theme {
        name: "Tokyo Night",
        dark: true,
        bg: hex(0x1a1b26),
        panel: hex(0x16161e),
        surface: hex(0x24283b),
        text: hex(0xc0caf5),
        weak: hex(0x7a83ad),
        accent: hex(0x7aa2f7),
        selection: hex(0x2f3549),
        marked: hex(0xe0af68),
        dir: hex(0x7dcfff),
        archive: hex(0xbb9af7),
        border: hex(0x2f3549),
        danger: hex(0xf7768e),
        syntax: "base16-ocean.dark",
        radius: 5,
        icons: IconPack::Matrix,
        ansi: Some(&TOKYO_NIGHT),
    },
    Theme {
        name: "GitHub Light",
        dark: false,
        bg: hex(0xffffff),
        panel: hex(0xf6f8fa),
        surface: hex(0xffffff),
        text: hex(0x1f2328),
        weak: hex(0x656d76),
        accent: hex(0x0969da),
        selection: hex(0xddf4ff),
        marked: hex(0xcf222e),
        dir: hex(0x0550ae),
        archive: hex(0x8250df),
        border: hex(0xd0d7de),
        danger: hex(0xcf222e),
        syntax: "InspiredGitHub",
        radius: 5,
        icons: IconPack::Matrix,
        ansi: Some(&GITHUB_LIGHT),
    },
    // ------------------------------------------------------------ community themes
    // Order: bg, panel, surface, text, weak, accent, selection, marked, dir, archive, border, danger.
    app(
        "GitHub Dark",
        true,
        [0x0d1117, 0x010409, 0x161b22, 0xe6edf3, 0x8b949e, 0x58a6ff, 0x1c2d44, 0xd29922, 0x58a6ff, 0xbc8cff, 0x30363d, 0xf85149],
        "base16-ocean.dark",
        &GITHUB_DARK,
    ),
    app(
        "One Dark",
        true,
        [0x282c34, 0x21252b, 0x2c313a, 0xabb2bf, 0x8a909c, 0x61afef, 0x3a4150, 0xe5c07b, 0x61afef, 0xc678dd, 0x3e4451, 0xe06c75],
        "base16-ocean.dark",
        &ONE_DARK,
    ),
    app(
        "One Light",
        false,
        [0xfafafa, 0xeaeaeb, 0xffffff, 0x383a42, 0x696c77, 0x4078f2, 0xdde6fc, 0x8a5d00, 0x2f63d9, 0xa626a4, 0xdbdbdc, 0xe45649],
        "InspiredGitHub",
        &ONE_LIGHT,
    ),
    app(
        "Tokyo Night Light",
        false,
        [0xd5d6db, 0xcbccd1, 0xe1e2e7, 0x343b58, 0x5d6178, 0x34548a, 0xbfc5d9, 0x6e470c, 0x34548a, 0x5a4a78, 0xb6b8c0, 0x8c4351],
        "base16-ocean.light",
        &TOKYO_NIGHT_LIGHT,
    ),
    app(
        "Catppuccin Latte",
        false,
        [0xeff1f5, 0xe6e9ef, 0xdce0e8, 0x4c4f69, 0x6c6f85, 0x8839ef, 0xd6d9ec, 0x8a5004, 0x1e66f5, 0x8839ef, 0xccd0da, 0xd20f39],
        "InspiredGitHub",
        &CATPPUCCIN_LATTE,
    ),
    app(
        "Catppuccin Frappé",
        true,
        [0x303446, 0x292c3c, 0x414559, 0xc6d0f5, 0xa5adce, 0xca9ee6, 0x51576d, 0xe5c890, 0x8caaee, 0xf4b8e4, 0x51576d, 0xe78284],
        "base16-ocean.dark",
        &CATPPUCCIN_FRAPPE,
    ),
    app(
        "Catppuccin Macchiato",
        true,
        [0x24273a, 0x1e2030, 0x363a4f, 0xcad3f5, 0xa5adcb, 0xc6a0f6, 0x494d64, 0xeed49f, 0x8aadf4, 0xf5bde6, 0x494d64, 0xed8796],
        "base16-ocean.dark",
        &CATPPUCCIN_MACCHIATO,
    ),
    app(
        "Rosé Pine",
        true,
        [0x191724, 0x1f1d2e, 0x26233a, 0xe0def4, 0x908caa, 0xc4a7e7, 0x403d52, 0xf6c177, 0x9ccfd8, 0xebbcba, 0x403d52, 0xeb6f92],
        "base16-mocha.dark",
        &ROSE_PINE,
    ),
    app(
        "Rosé Pine Moon",
        true,
        [0x232136, 0x2a273f, 0x393552, 0xe0def4, 0x908caa, 0xc4a7e7, 0x44415a, 0xf6c177, 0x9ccfd8, 0xea9a97, 0x44415a, 0xeb6f92],
        "base16-mocha.dark",
        &ROSE_PINE_MOON,
    ),
    app(
        "Rosé Pine Dawn",
        false,
        [0xfaf4ed, 0xf2e9e1, 0xfffaf3, 0x575279, 0x6e6a86, 0x907aa9, 0xdfdad9, 0x8d530c, 0x286983, 0x907aa9, 0xdfdad9, 0xb4637a],
        "InspiredGitHub",
        &ROSE_PINE_DAWN,
    ),
    app(
        "Kanagawa",
        true,
        [0x1f1f28, 0x16161d, 0x2a2a37, 0xdcd7ba, 0x9a9785, 0x7e9cd8, 0x2d4f67, 0xe6c384, 0x7fb4ca, 0x957fb8, 0x363646, 0xe82424],
        "base16-eighties.dark",
        &KANAGAWA,
    ),
    app(
        "Kanagawa Dragon",
        true,
        [0x181616, 0x12120f, 0x282727, 0xc5c9c5, 0x9e9b93, 0x8ba4b0, 0x393836, 0xc4b28a, 0x8ba4b0, 0xa292a3, 0x393836, 0xc4746e],
        "base16-eighties.dark",
        &KANAGAWA_DRAGON,
    ),
    app(
        "Everforest Dark",
        true,
        [0x2d353b, 0x232a2e, 0x343f44, 0xd3c6aa, 0x9da9a0, 0xa7c080, 0x475258, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x475258, 0xe67e80],
        "base16-eighties.dark",
        &EVERFOREST,
    ),
    app(
        "Everforest Light",
        false,
        [0xfdf6e3, 0xf4f0d9, 0xfffbef, 0x5c6a72, 0x6f7d74, 0x8da101, 0xe6e9c4, 0x8a5c00, 0x2f7fb0, 0xb04e95, 0xe0dcc7, 0xf85552],
        "InspiredGitHub",
        &EVERFOREST_LIGHT,
    ),
    app(
        "Ayu Light",
        false,
        [0xfcfcfc, 0xf3f4f5, 0xffffff, 0x5c6166, 0x6f747a, 0xfa8d3e, 0xe7eaed, 0x99600d, 0x2b7fc2, 0x8a5ebf, 0xe6e7e9, 0xe65050],
        "InspiredGitHub",
        &AYU_LIGHT,
    ),
    app(
        "Ayu Mirage",
        true,
        [0x1f2430, 0x1c212b, 0x242936, 0xcccac2, 0x8a93a5, 0xffcc66, 0x33415e, 0xffd173, 0x73d0ff, 0xdfbfff, 0x2f3646, 0xf28779],
        "base16-ocean.dark",
        &AYU_MIRAGE,
    ),
    app(
        "Ayu Dark",
        true,
        [0x0b0e14, 0x0d1017, 0x131721, 0xbfbdb6, 0x7d8590, 0xe6b450, 0x1f2a3d, 0xffb454, 0x59c2ff, 0xd2a6ff, 0x1e232b, 0xf07178],
        "base16-ocean.dark",
        &AYU_DARK,
    ),
    app(
        "Nord Light",
        false,
        [0xeceff4, 0xe5e9f0, 0xf8f9fb, 0x2e3440, 0x5b6478, 0x5e81ac, 0xd3dcea, 0x8a5a2b, 0x4c6f99, 0x8f6a8a, 0xd8dee9, 0xbf616a],
        "base16-ocean.light",
        &NORD_LIGHT,
    ),
    app(
        "Material",
        true,
        [0x263238, 0x1e272c, 0x2e3c43, 0xeeffff, 0x8fa4ae, 0x80cbc4, 0x37474f, 0xffcb6b, 0x82aaff, 0xc792ea, 0x37474f, 0xff5370],
        "base16-ocean.dark",
        &MATERIAL,
    ),
    app(
        "Material Ocean",
        true,
        [0x0f111a, 0x090b10, 0x1a1c25, 0xa6accd, 0x717cb4, 0x84ffff, 0x232838, 0xffcb6b, 0x82aaff, 0xc792ea, 0x1f2233, 0xff5370],
        "base16-ocean.dark",
        &MATERIAL_OCEAN,
    ),
];

/// Theme of a fresh install (also used for unknown theme names).
pub const DEFAULT: &str = "Breeze Light";

/// Pseudo theme name: follow the operating system (native look + light/dark).
pub const SYSTEM: &str = "System";

fn is_kde() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_uppercase().contains("KDE")
}

/// The native-looking theme for this OS and colour scheme.
pub fn native(dark: bool) -> &'static Theme {
    let family = if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(windows) {
        "Windows 11"
    } else if is_kde() {
        "Breeze"
    } else {
        "Adwaita"
    };
    by_name(&format!("{family} {}", if dark { "Dark" } else { "Light" }))
}

pub fn by_name(name: &str) -> &'static Theme {
    let name = renamed(name).unwrap_or(name);
    THEMES
        .iter()
        .find(|t| t.name == name)
        .or_else(|| CUSTOM.read().ok().and_then(|c| c.0.iter().copied().find(|t| t.name == name)))
        .or_else(|| THEMES.iter().find(|t| t.name == DEFAULT))
        .unwrap_or(&THEMES[0])
}

/// The theme an old name now stands for: themes renamed together with the app (Ferrum
/// Commander → Keymaker → Matrix Files → Matrix Finder).
/// Elegant Dark and Light (removed after 0.16) are now Breeze Dark and Light.
pub fn renamed(name: &str) -> Option<&'static str> {
    RENAMED.iter().find(|(old, _)| *old == name).map(|(_, new)| *new)
}

/// Old theme names → the theme they stand for now.
const RENAMED: [(&str, &str); 9] = [
    ("Ferrum Dark", "Breeze Dark"),
    ("Keymaker Dark", "Breeze Dark"),
    ("Matrix Dark", "Breeze Dark"),
    ("Elegant Dark", "Breeze Dark"),
    ("Ferrum Light", "Breeze Light"),
    ("Keymaker Light", "Breeze Light"),
    ("Matrix Light", "Breeze Light"),
    ("Elegant Light", "Breeze Light"),
    ("Matrix", "Digital Rain"),
];

/// Whether `name` is taken by a built-in theme or terminal scheme, an old name of one, or
/// "System" (a theme the user makes needs a name of its own, or another theme shows).
pub fn is_builtin(name: &str) -> bool {
    let n = name.trim();
    n.eq_ignore_ascii_case(SYSTEM)
        || RENAMED.iter().any(|(o, _)| o.eq_ignore_ascii_case(n))
        || THEMES.iter().chain(TERM_THEMES).any(|t| t.name.eq_ignore_ascii_case(n))
}

// ---------------------------------------------------------------- the user's themes

/// The code preview colour sets (syntect's own) a theme may pick.
pub const SYNTAX_THEMES: &[&str] =
    &["InspiredGitHub", "Solarized (light)", "base16-ocean.light", "Solarized (dark)", "base16-ocean.dark", "base16-eighties.dark", "base16-mocha.dark"];

/// The user's app themes and terminal schemes, registered by `set_custom`.
static CUSTOM: RwLock<(Vec<&'static Theme>, Vec<&'static Theme>)> = RwLock::new((Vec::new(), Vec::new()));

/// Every theme, name and colour set handed out as `&'static` so far: a theme that comes
/// back unchanged (most of them, each time the list is registered again) is reused, so
/// only an edit adds one. Editing a colour by dragging adds one per step, a few hundred
/// bytes each: nothing that adds up in a session.
#[derive(Default)]
struct Pool {
    themes: Vec<&'static Theme>,
    names: Vec<&'static str>,
    ansi: Vec<&'static [u32; 16]>,
}

static POOL: LazyLock<Mutex<Pool>> = LazyLock::new(Default::default);

impl Pool {
    fn name(&mut self, s: &str) -> &'static str {
        if let Some(n) = self.names.iter().find(|n| **n == s) {
            return n;
        }
        let n: &'static str = Box::leak(s.to_string().into_boxed_str());
        self.names.push(n);
        n
    }

    fn ansi(&mut self, a: [u32; 16]) -> &'static [u32; 16] {
        if let Some(x) = self.ansi.iter().find(|x| ***x == a) {
            return x;
        }
        let x: &'static [u32; 16] = Box::leak(Box::new(a));
        self.ansi.push(x);
        x
    }

    fn theme(&mut self, t: Theme) -> &'static Theme {
        if let Some(x) = self.themes.iter().find(|x| ***x == t) {
            return x;
        }
        let x: &'static Theme = Box::leak(Box::new(t));
        self.themes.push(x);
        x
    }
}

/// Parse `#rrggbb` (the `#` is optional).
pub fn parse_hex(s: &str) -> Option<Color32> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(s, 16).ok().map(hex)
}

/// A colour as `#rrggbb`.
pub fn to_hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

fn rgb(c: Color32) -> u32 {
    (c.r() as u32) << 16 | (c.g() as u32) << 8 | c.b() as u32
}

/// The 16 terminal colours of a saved theme, if all 16 are valid.
fn parse_ansi(list: &[String]) -> Option<[u32; 16]> {
    let v: Vec<u32> = list.iter().filter_map(|s| parse_hex(s)).map(rgb).collect();
    v.try_into().ok()
}

/// A theme the user made. A colour that does not parse keeps the one of the built-in
/// theme it falls back to (Breeze Light or Dark).
pub fn from_custom(c: &CustomTheme) -> Theme {
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    from_custom_in(&mut pool, c)
}

fn from_custom_in(pool: &mut Pool, c: &CustomTheme) -> Theme {
    let base = by_builtin(if c.dark { "Breeze Dark" } else { "Breeze Light" });
    let col = |s: &str, d: Color32| parse_hex(s).unwrap_or(d);
    Theme {
        name: pool.name(c.name.trim()),
        dark: c.dark,
        bg: col(&c.bg, base.bg),
        panel: col(&c.panel, base.panel),
        surface: col(&c.surface, base.surface),
        text: col(&c.text, base.text),
        weak: col(&c.weak, base.weak),
        accent: col(&c.accent, base.accent),
        selection: col(&c.selection, base.selection),
        marked: col(&c.marked, base.marked),
        dir: col(&c.dir, base.dir),
        archive: col(&c.archive, base.archive),
        border: col(&c.border, base.border),
        danger: col(&c.danger, base.danger),
        syntax: SYNTAX_THEMES.iter().find(|s| **s == c.syntax).copied().unwrap_or(base.syntax),
        radius: c.radius.min(12),
        icons: IconPack::from_key(&c.icons).unwrap_or(IconPack::Matrix),
        ansi: parse_ansi(&c.ansi).map(|a| pool.ansi(a)),
    }
}

/// A terminal colour scheme the user made; light or dark follows its background.
pub fn from_custom_term(c: &CustomTermTheme) -> Theme {
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    from_custom_term_in(&mut pool, c)
}

fn from_custom_term_in(pool: &mut Pool, c: &CustomTermTheme) -> Theme {
    let bg = parse_hex(&c.background).unwrap_or(Color32::from_rgb(0x1b, 0x1e, 0x20));
    let dark = luminance(bg) < 0.3;
    let fg = parse_hex(&c.foreground).unwrap_or(if dark { Color32::from_rgb(0xfc, 0xfc, 0xfc) } else { Color32::from_rgb(0x23, 0x26, 0x29) });
    let accent = parse_hex(&c.cursor).unwrap_or(Color32::from_rgb(0x3d, 0xae, 0xe9));
    let ansi = pool.ansi(parse_ansi(&c.ansi).unwrap_or(if dark { BREEZE_DARK } else { BREEZE_LIGHT }));
    let mut t = scheme("", dark, rgb(bg), rgb(fg), rgb(accent), ansi);
    t.name = pool.name(c.name.trim());
    t
}

/// A theme as the user's own, named `name`, to edit (made from the theme shown).
pub fn to_custom(t: &Theme, name: &str) -> CustomTheme {
    CustomTheme {
        name: name.to_string(),
        dark: t.dark,
        bg: to_hex(t.bg),
        panel: to_hex(t.panel),
        surface: to_hex(t.surface),
        text: to_hex(t.text),
        weak: to_hex(t.weak),
        accent: to_hex(t.accent),
        selection: to_hex(t.selection),
        marked: to_hex(t.marked),
        dir: to_hex(t.dir),
        archive: to_hex(t.archive),
        border: to_hex(t.border),
        danger: to_hex(t.danger),
        radius: t.radius,
        icons: t.icons.key().to_string(),
        syntax: t.syntax.to_string(),
        ansi: t.ansi.map(|a| a.iter().map(|c| to_hex(hex(*c))).collect()).unwrap_or_default(),
    }
}

/// A terminal scheme as the user's own, named `name`.
pub fn to_custom_term(t: &Theme, name: &str) -> CustomTermTheme {
    let ansi = t.ansi.copied().unwrap_or(if t.dark { BREEZE_DARK } else { BREEZE_LIGHT });
    CustomTermTheme {
        name: name.to_string(),
        background: to_hex(t.bg),
        foreground: to_hex(t.text),
        cursor: to_hex(t.accent),
        ansi: ansi.iter().map(|c| to_hex(hex(*c))).collect(),
    }
}

/// Register the user's themes (after loading or changing the settings).
pub fn set_custom(app: &[CustomTheme], term: &[CustomTermTheme]) {
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let mut a: Vec<&'static Theme> = Vec::with_capacity(app.len());
    for c in app {
        let t = from_custom_in(&mut pool, c);
        a.push(pool.theme(t));
    }
    let mut b: Vec<&'static Theme> = Vec::with_capacity(term.len());
    for c in term {
        let t = from_custom_term_in(&mut pool, c);
        b.push(pool.theme(t));
    }
    drop(pool);
    *CUSTOM.write().unwrap_or_else(|e| e.into_inner()) = (a, b);
}

/// The user's app themes, in their order.
pub fn custom() -> Vec<&'static Theme> {
    CUSTOM.read().map(|c| c.0.clone()).unwrap_or_default()
}

/// The user's terminal schemes, in their order.
pub fn custom_term() -> Vec<&'static Theme> {
    CUSTOM.read().map(|c| c.1.clone()).unwrap_or_default()
}

fn by_builtin(name: &str) -> &'static Theme {
    THEMES.iter().find(|t| t.name == name).unwrap_or(&THEMES[0])
}

impl Theme {
    /// Readable text colour on top of the accent colour.
    pub fn on_accent(&self) -> Color32 {
        let [r, g, b, _] = self.accent.to_array();
        let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        if lum > 150.0 {
            Color32::from_rgb(20, 20, 24)
        } else {
            Color32::WHITE
        }
    }

    /// Slightly emphasised text (headings, current item).
    pub fn strong(&self) -> Color32 {
        if self.dark {
            self.text.lerp_to_gamma(Color32::WHITE, 0.5)
        } else {
            self.text.lerp_to_gamma(Color32::BLACK, 0.5)
        }
    }

    /// `c` drawn on the cursor row (`selection`), moved toward black or white until it
    /// reads there: 4.5:1 for the current name (`strong`), 3:1 for the rest (marked,
    /// hidden, sizes). Most themes need no change; Classic Commander's cyan bar does.
    pub fn on_cursor(&self, c: Color32) -> Color32 {
        let min = if c == self.strong() { 4.5 } else { 3.0 };
        if contrast(c, self.selection) >= min {
            return c;
        }
        for i in 1..=20 {
            let t = i as f32 / 20.0;
            for to in [Color32::BLACK, Color32::WHITE] {
                let moved = c.lerp_to_gamma(to, t);
                if contrast(moved, self.selection) >= min {
                    return moved;
                }
            }
        }
        if contrast(Color32::BLACK, self.selection) > contrast(Color32::WHITE, self.selection) {
            Color32::BLACK
        } else {
            Color32::WHITE
        }
    }

    /// Hover background for list rows and flat buttons.
    pub fn hover(&self) -> Color32 {
        self.bg.lerp_to_gamma(self.surface, 0.55)
    }

    /// A tab under the pointer, as in a browser: clearly darker than the tab bar
    /// (lighter on a dark theme), and still apart from the selected tab.
    pub fn tab_hover(&self) -> Color32 {
        self.panel.lerp_to_gamma(self.text, if self.dark { 0.13 } else { 0.10 })
    }
}

/// Relative luminance of a colour (WCAG).
fn luminance(c: Color32) -> f32 {
    let f = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
}

/// WCAG contrast ratio of two colours (1 to 21).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

pub fn apply(ctx: &egui::Context, t: &Theme) {
    use egui::{CornerRadius, FontFamily, FontId, Margin, Shadow, TextStyle};
    let mut v = if t.dark { Visuals::dark() } else { Visuals::light() };
    v.override_text_color = Some(t.text);
    v.panel_fill = t.panel;
    v.window_fill = t.panel;
    v.extreme_bg_color = t.surface;
    v.faint_bg_color = t.surface;
    v.code_bg_color = t.surface;
    v.hyperlink_color = t.accent;
    v.warn_fg_color = t.marked;
    v.error_fg_color = t.danger;
    v.selection.bg_fill = t.accent.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0_f32, t.accent);
    v.window_stroke = Stroke::new(1.0_f32, t.border);
    let rr = t.radius;
    v.window_corner_radius = CornerRadius::same((rr * 2).max(6));
    v.menu_corner_radius = CornerRadius::same((rr + 4).max(5));
    let shadow_alpha = if t.dark { 90 } else { 28 };
    v.window_shadow = Shadow { offset: [0, 12], blur: 32, spread: 0, color: Color32::from_black_alpha(shadow_alpha) };
    v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(shadow_alpha) };
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = false;
    v.striped = false;
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);

    let r6 = CornerRadius::same(rr);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = t.panel;
    w.noninteractive.weak_bg_fill = t.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, t.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, t.text);
    w.noninteractive.corner_radius = r6;

    w.inactive.bg_fill = t.surface;
    w.inactive.weak_bg_fill = t.surface;
    w.inactive.fg_stroke = Stroke::new(1.0_f32, t.text);
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.corner_radius = r6;

    let hover = t.surface.lerp_to_gamma(t.text, 0.08);
    w.hovered.bg_fill = hover;
    w.hovered.weak_bg_fill = hover;
    w.hovered.fg_stroke = Stroke::new(1.0_f32, t.strong());
    w.hovered.bg_stroke = Stroke::NONE;
    w.hovered.corner_radius = r6;
    w.hovered.expansion = 0.0;

    let press = t.surface.lerp_to_gamma(t.text, 0.16);
    w.active.bg_fill = press;
    w.active.weak_bg_fill = press;
    // `active.fg_stroke` doubles as egui's "strong" text colour.
    w.active.fg_stroke = Stroke::new(1.0_f32, t.strong());
    w.active.bg_stroke = Stroke::NONE;
    w.active.corner_radius = r6;
    w.active.expansion = 0.0;

    w.open.bg_fill = t.surface;
    w.open.weak_bg_fill = t.surface;
    w.open.fg_stroke = Stroke::new(1.0_f32, t.text);
    w.open.bg_stroke = Stroke::NONE;
    w.open.corner_radius = r6;

    // Pin egui's own light/dark preference so OS changes don't swap in egui's default style.
    ctx.set_theme(if t.dark { egui::Theme::Dark } else { egui::Theme::Light });
    let mut style = (*ctx.global_style()).clone();
    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    style.spacing.interact_size = egui::vec2(24.0, 26.0);
    style.spacing.window_margin = Margin::same(16);
    style.spacing.menu_margin = Margin::same(6);
    style.spacing.indent = 14.0;
    style.spacing.combo_width = 120.0;
    style.spacing.scroll.bar_width = 6.0;
    style.spacing.scroll.floating = true;
    // No fading at the edges of scrolled lists: the file lists paint their own background.
    style.spacing.scroll.fade.strength = 0.0;
    style.text_styles = [
        // Default text size: 14 px (Settings → UI scale zooms everything).
        (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Heading, FontId::new(20.0, FontFamily::Name("bold".into()))),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
    ]
    .into();
    // The same style whichever light/dark mode egui is in (the OS, or a test harness, may
    // switch it): popups and menus would otherwise get egui's own.
    ctx.set_style_of(egui::Theme::Light, style.clone());
    ctx.set_style_of(egui::Theme::Dark, style);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_schemes_are_distinct_and_readable() {
        let mut names: Vec<&str> = TERM_THEMES.iter().map(|t| t.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TERM_THEMES.len(), "unique names");
        for t in TERM_THEMES {
            let (a, b) = (luminance(t.text), luminance(t.bg));
            let contrast = (a.max(b) + 0.05) / (a.min(b) + 0.05);
            assert!(contrast >= 4.0, "{}: text contrast {contrast}", t.name);
            assert_eq!(t.dark, b < 0.3, "{}: dark flag", t.name);
            assert!(t.ansi.is_some());
        }
    }

    #[test]
    fn hovered_tabs_stand_out() {
        let contrast = |a: Color32, b: Color32| {
            let (a, b) = (luminance(a), luminance(b));
            (a.max(b) + 0.05) / (a.min(b) + 0.05)
        };
        for t in THEMES {
            let h = t.tab_hover();
            // Clearly darker than the tab bar (lighter on a dark theme), as in a browser.
            assert!(contrast(h, t.panel) >= 1.12, "{}: hover {:.3} on the bar", t.name, contrast(h, t.panel));
            assert_eq!(luminance(h) < luminance(t.panel), !t.dark, "{}: darker on light, lighter on dark", t.name);
            // The title, drawn strong while hovered, stays readable.
            assert!(contrast(h, t.strong()) >= 4.5, "{}: title {:.3}", t.name, contrast(h, t.strong()));
        }
    }

    /// File names stay readable in every app theme: plain, marked and hidden names, on
    /// a row and under the cursor (WCAG contrast: 4.5 for text, 3 where a row is lit).
    #[test]
    fn file_list_colours_are_readable() {
        let mut bad = vec![];
        for t in THEMES {
            // A marked row has a faint tint of the marked colour, and the cursor row
            // draws its text through `on_cursor` (as pane.rs does).
            let tinted = t.bg.lerp_to_gamma(t.marked, if t.dark { 0.10 } else { 0.08 });
            let checks = [
                ("text on bg", t.text, t.bg, 4.5),
                ("marked on its row", t.marked, tinted, 4.5),
                ("weak on bg", t.weak, t.bg, 3.0),
                ("current name on the cursor", t.on_cursor(t.strong()), t.selection, 4.5),
                ("marked on the cursor", t.on_cursor(t.marked), t.selection, 3.0),
                ("weak on the cursor", t.on_cursor(t.weak), t.selection, 3.0),
            ];
            for (what, fg, bg, min) in checks {
                let c = contrast(fg, bg);
                if c < min {
                    bad.push(format!("{}: {what} {c:.2} < {min}", t.name));
                }
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    #[test]
    fn the_users_themes_are_found_by_name() {
        let mut mine = to_custom(by_name("Nord"), "Test Mine");
        let term = to_custom_term(term_by_name("Dracula"), "Test Night");
        set_custom(std::slice::from_ref(&mine), std::slice::from_ref(&term));
        let a = by_name("Test Mine");
        assert_eq!((a.name, a.bg, a.ansi), ("Test Mine", by_name("Nord").bg, by_name("Nord").ansi), "a copy of Nord");
        assert_eq!(term_by_name("Test Night").bg, term_by_name("Dracula").bg);
        assert_eq!(term_by_name("Test Night").ansi, term_by_name("Dracula").ansi);
        // Registered again unchanged: the same theme (nothing new is kept); changed: a new one.
        set_custom(std::slice::from_ref(&mine), std::slice::from_ref(&term));
        assert!(std::ptr::eq(a, by_name("Test Mine")));
        mine.accent = "#ff0000".into();
        mine.text = "not a colour".into();
        mine.ansi.truncate(3);
        set_custom(std::slice::from_ref(&mine), &[]);
        let b = by_name("Test Mine");
        assert_eq!(b.accent, hex(0xff0000));
        assert_eq!(b.text, by_name("Breeze Dark").text, "a colour that does not parse falls back");
        assert_eq!(b.ansi, None, "not 16 colours: the general set");
        assert_eq!(term_by_name("Test Night").name, DEFAULT, "removed: an unknown name");
        set_custom(&[], &[]);
        assert_eq!(by_name("Test Mine").name, DEFAULT);
    }

    #[test]
    fn colours_read_and_write_as_hex() {
        assert_eq!(parse_hex("#1a2B3c"), Some(Color32::from_rgb(0x1a, 0x2b, 0x3c)));
        assert_eq!(parse_hex(" 1a2b3c "), Some(Color32::from_rgb(0x1a, 0x2b, 0x3c)));
        for bad in ["", "#fff", "#1a2b3g", "#1a2b3c4", "ééé"] {
            assert_eq!(parse_hex(bad), None, "{bad}");
        }
        assert_eq!(to_hex(Color32::from_rgb(1, 2, 255)), "#0102ff");
        // A built-in theme made the user's keeps every colour.
        for t in THEMES {
            let c = to_custom(t, "Copy");
            let back = from_custom(&c);
            assert_eq!(
                (back.bg, back.text, back.selection, back.ansi, back.syntax, back.icons, back.radius),
                (t.bg, t.text, t.selection, t.ansi, t.syntax, t.icons, t.radius),
                "{}",
                t.name
            );
        }
    }

    #[test]
    fn theme_names_are_unique() {
        let names: Vec<String> = THEMES.iter().chain(TERM_THEMES).map(|t| t.name.to_lowercase()).collect();
        let mut app: Vec<&str> = THEMES.iter().map(|t| t.name).collect();
        app.sort_unstable();
        app.dedup();
        assert_eq!(app.len(), THEMES.len(), "app themes");
        assert!(names.iter().all(|n| is_builtin(n)));
        // Names that stand for another theme are taken too.
        for n in ["System", "system", "Matrix", "Matrix Dark", "ferrum light", "Elegant Dark"] {
            assert!(is_builtin(n), "{n}");
        }
        assert!(!is_builtin("My Matrix"));
        let got = crate::config::unique_theme_names(&["Nord".into(), "mine".into(), " Mine ".into(), "".into(), "Mine 2".into()], "Custom");
        assert_eq!(got, ["Nord 2", "mine", "Mine 2", "Custom", "Mine 2 2"]);
    }

    #[test]
    fn terminal_theme_lookup() {
        assert_eq!(term_by_name("Tokyo Night").name, "Tokyo Night");
        // An app theme that is no scheme, from older settings.
        assert_eq!(term_by_name("Adwaita Light").name, "Adwaita Light");
        // Removed themes: the one that took their place.
        assert_eq!(by_name("Elegant Dark").name, "Breeze Dark");
        assert_eq!(by_name("Matrix Light").name, "Breeze Light");
        assert_eq!(term_by_name("Elegant Light").name, "Breeze Light");
        assert_eq!(by_name("Dracula").ansi, term_by_name("Dracula").ansi);
    }
}
