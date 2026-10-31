//! Whether a running app looks like a game, from facts each OS reads without capture (detection
//! only suggests: "{game} is running. Record it?"; it never starts anything). The rules are platform-neutral; each
//! backend fills in the facts it has:
//! - macOS: `Info.plist`'s `LSApplicationCategoryType` and `GCSupportsGameMode` or `LSSupportsGameMode`;
//! - Windows: the process image path (Steam, Epic Games and GOG Galaxy libraries);
//! - Linux: the desktop file's `Categories` (and the executable's path, for Steam's).
//!
//! The launchers themselves, and the tools Steam installs beside games, are never suggested.

/// What the OS says about a running app.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppFacts {
    pub app_id: String,
    /// macOS's `LSApplicationCategoryType` ("public.app-category.action-games").
    pub category: Option<String>,
    /// `GCSupportsGameMode` or `LSSupportsGameMode` (macOS).
    pub game_mode: bool,
    /// The executable's path.
    pub exe: Option<String>,
    /// The desktop file's `Categories` (freedesktop: "Game", "ArcadeGame", …).
    pub desktop_categories: Vec<String>,
}

/// Folders a game's executable lives under. Paths match in any case, with `/` and `\` alike, so one table serves every
/// OS (Steam's library is `steamapps/common/` on Linux and macOS too).
const GAME_LIBRARIES: [&str; 3] = ["steamapps/common/", "GOG Galaxy/Games/", "Epic Games/"];

/// Inside those folders but not games: Epic's own launcher, services, Unreal
/// Editor (`UE_5.4/…`) and DirectX redistributable, and the tools and runtimes Steam installs beside games.
const NOT_GAMES: [&str; 12] = [
    "Epic Games/Launcher/",
    "Epic Games/Epic Online Services/",
    "Epic Games/UE_",
    "Epic Games/DirectXRedist/",
    "steamapps/common/wallpaper_engine/",
    "steamapps/common/Wallpaper Engine/",
    "steamapps/common/SteamVR/",
    "steamapps/common/OBS Studio/",
    "steamapps/common/Blender/",
    "steamapps/common/Proton",
    "steamapps/common/SteamLinuxRuntime",
    "steamapps/common/Steamworks Shared/",
];

/// The launchers themselves, by app id (a bundle id or desktop-file id) or by executable name without its extension
/// (any case). Their own metadata calls them games (`steam.desktop`'s `Categories=…;Game;`, Lutris, Heroic).
const LAUNCHERS: [&str; 12] = [
    "steam",
    "steam.desktop",
    "steamwebhelper",
    "com.valvesoftware.steam",
    "GalaxyClient",
    "com.gog.galaxy",
    "EpicGamesLauncher",
    "com.epicgames.EpicGamesLauncher",
    "lutris",
    "net.lutris.Lutris",
    "heroic",
    "com.heroicgameslauncher.hgl",
];

pub fn looks_like_game(f: &AppFacts) -> bool {
    if is_launcher(f) {
        return false;
    }
    f.category
        .as_deref()
        .is_some_and(|c| contains_ignore_case(c, "games"))
        || f.game_mode
        || f.exe.as_deref().is_some_and(|exe| {
            GAME_LIBRARIES.iter().any(|lib| contains_path(exe, lib))
                && !NOT_GAMES.iter().any(|tool| contains_path(exe, tool))
        })
        || f.desktop_categories
            .iter()
            .any(|c| c.eq_ignore_ascii_case("Game"))
}

fn is_launcher(f: &AppFacts) -> bool {
    let exe_name = f.exe.as_deref().map(|exe| {
        let name = exe.rsplit(['/', '\\']).next().unwrap_or(exe);
        name.rsplit_once('.').map_or(name, |(stem, _)| stem)
    });
    LAUNCHERS.iter().any(|l| {
        f.app_id.eq_ignore_ascii_case(l) || exe_name.is_some_and(|n| n.eq_ignore_ascii_case(l))
    })
}

/// Whether `needle` is in `haystack`, in any ASCII case (the macOS backend's virtual-device names use it too).
pub(crate) fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

/// `contains_ignore_case`, with `/` and `\` the same.
fn contains_path(haystack: &str, needle: &str) -> bool {
    let fold = |c: u8| {
        if c == b'\\' {
            b'/'
        } else {
            c.to_ascii_lowercase()
        }
    };
    haystack.as_bytes().windows(needle.len()).any(|w| {
        w.iter()
            .zip(needle.as_bytes())
            .all(|(&a, &b)| fold(a) == fold(b))
    })
}
