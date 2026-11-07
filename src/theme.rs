//! Colour themes. Each theme maps a small palette onto egui visuals.
use crate::icons::IconPack;
use egui::{Color32, Stroke, Visuals};

#[derive(Clone, Copy)]
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

/// The colour schemes offered for terminal profiles.
pub const TERM_THEMES: &[Theme] = &[
    scheme("Breeze Light", false, 0xffffff, 0x232629, 0x3daee9, &BREEZE_LIGHT),
    scheme("Breeze Dark", true, 0x1b1e20, 0xfcfcfc, 0x3daee9, &BREEZE_DARK),
    scheme("GitHub Light", false, 0xffffff, 0x24292f, 0x0969da, &GITHUB_LIGHT),
    scheme("GitHub Dark", true, 0x0d1117, 0xc9d1d9, 0x58a6ff, &GITHUB_DARK),
    scheme("One Light", false, 0xfafafa, 0x383a42, 0x526fff, &ONE_LIGHT),
    scheme("One Dark", true, 0x282c34, 0xabb2bf, 0x528bff, &ONE_DARK),
    scheme("Catppuccin Latte", false, 0xeff1f5, 0x4c4f69, 0x8839ef, &CATPPUCCIN_LATTE),
    scheme("Catppuccin Mocha", true, 0x1e1e2e, 0xcdd6f4, 0xcba6f7, &CATPPUCCIN_MOCHA),
    scheme("Solarized Light", false, 0xfdf6e3, 0x657b83, 0x268bd2, &SOLARIZED),
    scheme("Solarized Dark", true, 0x002b36, 0x839496, 0x2aa198, &SOLARIZED),
    scheme("Gruvbox Light", false, 0xfbf1c7, 0x3c3836, 0xaf3a03, &GRUVBOX_LIGHT),
    scheme("Gruvbox Dark", true, 0x282828, 0xebdbb2, 0xfe8019, &GRUVBOX_DARK),
    scheme("Dracula", true, 0x282a36, 0xf8f8f2, 0xbd93f9, &DRACULA),
    scheme("Nord", true, 0x2e3440, 0xd8dee9, 0x88c0d0, &NORD),
    scheme("Tokyo Night", true, 0x1a1b26, 0xc0caf5, 0x7aa2f7, &TOKYO_NIGHT),
    scheme("Monokai", true, 0x272822, 0xf8f8f2, 0xa6e22e, &MONOKAI),
    scheme("Rosé Pine", true, 0x191724, 0xe0def4, 0xc4a7e7, &ROSE_PINE),
    scheme("Kanagawa", true, 0x1f1f28, 0xdcd7ba, 0x7e9cd8, &KANAGAWA),
    scheme("Everforest", true, 0x2d353b, 0xd3c6aa, 0xa7c080, &EVERFOREST),
    scheme("Green Phosphor", true, 0x0a120a, 0x7dff9c, 0x00d75f, &GREEN_PHOSPHOR),
    scheme("Amber Phosphor", true, 0x140e04, 0xffc24a, 0xffa200, &AMBER_PHOSPHOR),
];

/// A terminal profile's colour theme: a scheme, or (from older settings) an app theme.
pub fn term_by_name(name: &str) -> &'static Theme {
    TERM_THEMES.iter().find(|t| t.name == name).unwrap_or_else(|| by_name(name))
}

pub const THEMES: &[Theme] = &[
    Theme {
        name: "Elegant Dark",
        dark: true,
        bg: hex(0x131418),
        panel: hex(0x18191e),
        surface: hex(0x23252c),
        text: hex(0xe4e5ea),
        weak: hex(0x8b8e99),
        accent: hex(0x9ea8ff),
        selection: hex(0x262a40),
        marked: hex(0xe6c486),
        dir: hex(0x8fa3e6),
        archive: hex(0xbfa3ea),
        border: hex(0x26272e),
        danger: hex(0xef7f85),
        syntax: "base16-ocean.dark",
        radius: 5,
        icons: IconPack::Matrix,
        ansi: None,
    },
    Theme {
        name: "Elegant Light",
        dark: false,
        bg: hex(0xffffff),
        panel: hex(0xf6f6f8),
        surface: hex(0xebebf0),
        text: hex(0x1c1d23),
        weak: hex(0x6a6d78),
        accent: hex(0x5360d8),
        selection: hex(0xe5e8fb),
        marked: hex(0xa4661c),
        dir: hex(0x5a72d0),
        archive: hex(0x8752c4),
        border: hex(0xe2e2e8),
        danger: hex(0xcf3e3e),
        syntax: "InspiredGitHub",
        radius: 5,
        icons: IconPack::Matrix,
        ansi: None,
    },

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
        marked: hex(0xe8890c),
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
        marked: hex(0xc64600),
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
        marked: hex(0xc25e00),
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
        marked: hex(0xf67400),
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
        marked: hex(0xcb4b16),
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
        weak: hex(0x93a1a1),
        accent: hex(0xb58900),
        selection: hex(0xe3dcc6),
        marked: hex(0xcb4b16),
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
    // Themes renamed together with the app (Ferrum Commander → Keymaker → Matrix Files → Matrix Finder).
    let name = match name {
        "Ferrum Dark" | "Keymaker Dark" | "Matrix Dark" => "Elegant Dark",
        "Ferrum Light" | "Keymaker Light" | "Matrix Light" => "Elegant Light",
        "Matrix" => "Digital Rain",
        other => other,
    };
    THEMES
        .iter()
        .find(|t| t.name == name)
        .or_else(|| THEMES.iter().find(|t| t.name == DEFAULT))
        .unwrap_or(&THEMES[0])
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

    /// Hover background for list rows and flat buttons.
    pub fn hover(&self) -> Color32 {
        self.bg.lerp_to_gamma(self.surface, 0.55)
    }
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
    let mut style = (*ctx.style()).clone();
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
    style.text_styles = [
        // Default text size: 14 px (Settings → UI scale zooms everything).
        (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Heading, FontId::new(20.0, FontFamily::Name("bold".into()))),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
    ]
    .into();
    ctx.set_style(style);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: Color32) -> f32 {
        let f = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }

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
    fn terminal_theme_lookup() {
        assert_eq!(term_by_name("Tokyo Night").name, "Tokyo Night");
        // An app theme that is no scheme, from older settings.
        assert_eq!(term_by_name("Elegant Light").name, "Elegant Light");
        assert_eq!(by_name("Dracula").ansi, term_by_name("Dracula").ansi);
    }
}
