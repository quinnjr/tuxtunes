//! Canonical genre names and the umbrella each one files under.
//!
//! Two vocabularies meet here. MusicBrainz genres are lowercase and
//! specific ("melodic death metal"); the library's existing tags are
//! whatever iTunes, Amazon and a decade of rippers left behind ("Death
//! Metal/Black Metal", "JPop", "atrilli.net"). Both are folded into one
//! title-cased canonical form, and every canonical genre maps to one of
//! a dozen umbrellas that become sidebar folders.

use serde::{Deserialize, Serialize};

/// The folder a specific genre files under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Umbrella {
    Metal,
    Alternative,
    Rock,
    Electronic,
    HipHop,
    Pop,
    Jazz,
    Classical,
    Soundtrack,
    Folk,
    NewAge,
    Other,
}

impl Umbrella {
    pub const ALL: [Umbrella; 12] = [
        Umbrella::Metal,
        Umbrella::Alternative,
        Umbrella::Rock,
        Umbrella::Electronic,
        Umbrella::HipHop,
        Umbrella::Pop,
        Umbrella::Jazz,
        Umbrella::Classical,
        Umbrella::Soundtrack,
        Umbrella::Folk,
        Umbrella::NewAge,
        Umbrella::Other,
    ];

    /// Sidebar folder name.
    pub fn display_name(self) -> &'static str {
        match self {
            Umbrella::Metal => "Metal",
            Umbrella::Alternative => "Alternative",
            Umbrella::Rock => "Rock",
            Umbrella::Electronic => "Electronic",
            Umbrella::HipHop => "Hip-Hop",
            Umbrella::Pop => "Pop",
            Umbrella::Jazz => "Jazz & Blues",
            Umbrella::Classical => "Classical",
            Umbrella::Soundtrack => "Soundtrack",
            Umbrella::Folk => "Folk & Country",
            Umbrella::NewAge => "New Age",
            Umbrella::Other => "Other",
        }
    }

    /// Inverse of [`display_name`](Self::display_name); used when the
    /// map file carries a hand-edited umbrella.
    pub fn from_display_name(s: &str) -> Option<Umbrella> {
        Umbrella::ALL
            .iter()
            .copied()
            .find(|u| u.display_name().eq_ignore_ascii_case(s.trim()))
    }
}

/// Exact-match aliases for existing tags, keyed by lowercased trimmed
/// input. Anything not listed goes through generic title-casing.
const ALIASES: &[(&str, &str)] = &[
    ("jpop", "J-Pop"),
    ("j-pop", "J-Pop"),
    ("domestic(j-pops)", "J-Pop"),
    ("j-rock", "J-Rock"),
    ("k-pop", "K-Pop"),
    ("k pop", "K-Pop"),
    ("j-core", "J-Core"),
    ("death metal/black metal", "Death Metal"),
    ("jungle/drum'n'bass", "Drum and Bass"),
    ("drum & bass", "Drum and Bass"),
    ("drum and bass", "Drum and Bass"),
    ("drum n bass", "Drum and Bass"),
    ("dnb", "Drum and Bass"),
    ("dubstep", "Dubstep"),
    ("hip-hop/rap", "Hip Hop"),
    ("hip-hop", "Hip Hop"),
    ("hip hop", "Hip Hop"),
    ("hip hop / trap", "Hip Hop"),
    ("rap", "Hip Hop"),
    ("video game", "Video Game Music"),
    ("game", "Video Game Music"),
    ("video game soundtrack", "Video Game Music"),
    ("video game music", "Video Game Music"),
    ("electronica", "Electronic"),
    ("electronica/dance", "Electronic"),
    ("psytrance, goa", "Psytrance"),
    ("prog-rock/art rock", "Progressive Rock"),
    ("prog rock", "Progressive Rock"),
    ("alternative & punk", "Alternative"),
    ("alternative rock", "Alternative Rock"),
    ("christian & gospel", "Gospel"),
    ("gospel & religious", "Gospel"),
    ("books & spoken", "Spoken Word"),
    ("sci fi & fantasy", "Spoken Word"),
    ("kids & young adults", "Spoken Word"),
    ("chill out", "Chillout"),
    ("hardcore", "Hardcore"),
    ("heavy metal", "Heavy Metal"),
    ("neo-classical djent", "Djent"),
    ("classical crossover", "Classical Crossover"),
    ("singer/songwriter", "Singer-Songwriter"),
    ("r&b", "R&B"),
    ("rnb", "R&B"),
];

/// Tags that carry no genre information at all.
const JUNK: &[&str] = &[
    "",
    "other",
    "unclassifiable",
    "unknown",
    "crystalize",
    "k theory",
    "misc",
    "general",
    "default",
    "none",
];

/// Fold an existing library tag into its canonical form, or `None`
/// when the tag is junk (empty, numeric, a URL, a known placeholder).
pub fn normalize_tag(raw: &str) -> Option<String> {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = collapsed.to_lowercase();
    if JUNK.contains(&lower.as_str()) {
        return None;
    }
    if lower
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == ' ')
    {
        return None;
    }
    if lower.starts_with("www.")
        || lower.starts_with("http")
        || [".net", ".com", ".org", ".io", ".fm", ".co.uk"]
            .iter()
            .any(|tld| lower.ends_with(tld))
    {
        // "atrilli.net", "www.example.com"
        return None;
    }
    if let Some((_, canon)) = ALIASES.iter().find(|(k, _)| *k == lower) {
        return Some((*canon).to_string());
    }
    Some(title_case(&collapsed))
}

/// Canonical form of a MusicBrainz genre name.
pub fn canonical_genre(mb_tag: &str) -> String {
    normalize_tag(mb_tag).unwrap_or_else(|| title_case(mb_tag.trim()))
}

/// Title-case every word except connectives after the first, keeping
/// hyphenated parts ("post-hardcore" → "Post-Hardcore") and preserving
/// words that already carry internal capitals ("EDM", "IDM", "EBM").
fn title_case(s: &str) -> String {
    const SMALL: &[&str] = &["and", "of", "the", "n", "a", "in", "on", "to"];
    s.split(' ')
        .enumerate()
        .map(|(i, word)| {
            let lower = word.to_lowercase();
            if i > 0 && SMALL.contains(&lower.as_str()) {
                return lower;
            }
            if word.len() <= 3 && word.chars().all(|c| c.is_ascii_uppercase()) {
                return word.to_string();
            }
            word.split('-').map(cap_first).collect::<Vec<_>>().join("-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn cap_first(part: &str) -> String {
    let mut chars = part.chars();
    match chars.next() {
        Some(first) => {
            let rest: String = chars.collect::<String>().to_lowercase();
            first.to_uppercase().collect::<String>() + &rest
        }
        None => String::new(),
    }
}

/// Ordered keyword table. The first umbrella whose keyword appears in
/// the lowercased genre wins, so specific families (metal, punk) are
/// listed before the broad words they contain ("rock", "pop") and
/// "industrial metal" lands in Metal before "industrial" can claim it
/// for Electronic.
const UMBRELLA_KEYWORDS: &[(Umbrella, &[&str])] = &[
    // Compounds that a broad keyword below would otherwise misfile.
    (
        Umbrella::Electronic,
        &[
            "hardcore techno",
            "happy hardcore",
            "uk hardcore",
            "bouncy hardcore",
            "gabber",
            "digital hardcore",
        ],
    ),
    (
        Umbrella::Rock,
        &[
            "garage rock",
            "symphonic rock",
            "industrial rock",
            "dance-rock",
            "dance rock",
            "pop rock",
        ],
    ),
    (Umbrella::Other, &["dancehall", "reggae"]),
    (
        Umbrella::Soundtrack,
        &[
            "soundtrack",
            "video game",
            "game music",
            "chiptune",
            "anime",
            "film score",
            "score",
            "musical",
            "broadway",
            "stage & screen",
            "theatre",
        ],
    ),
    (
        Umbrella::Metal,
        &[
            "metal",
            "grindcore",
            "deathcore",
            "djent",
            "thrash",
            "doom",
            "sludge",
            "black metal",
            "death metal",
            "nu metal",
            "power metal",
        ],
    ),
    (
        Umbrella::Alternative,
        &[
            "punk",
            "emo",
            "screamo",
            "hardcore",
            "post-hardcore",
            "indie",
            "alternative",
            "grunge",
            "shoegaze",
            "new wave",
            "goth",
            "ska",
            "noise rock",
            "post-rock",
            "math rock",
            "j-rock",
            "pop punk",
        ],
    ),
    (
        Umbrella::HipHop,
        &["hip hop", "rap", "trap", "grime", "drill"],
    ),
    (
        Umbrella::Electronic,
        &[
            "electro",
            "techno",
            "house",
            "trance",
            "dubstep",
            "drum and bass",
            "jungle",
            "edm",
            "idm",
            "ebm",
            "ambient",
            "synth",
            "industrial",
            "breakbeat",
            "breakcore",
            "eurobeat",
            "downtempo",
            "trip hop",
            "glitch",
            "hardstyle",
            "makina",
            "garage",
            "vocaloid",
            "dance",
            "chillout",
            "psytrance",
            "j-core",
            "big beat",
            "future bass",
            "drum & bass",
            "moombahton",
            "electronica",
        ],
    ),
    (
        Umbrella::Classical,
        &[
            "classical",
            "orchestral",
            "baroque",
            "opera",
            "symphon",
            "neoclassical",
            "chamber",
            "choral",
            "romantic",
            "contemporary classical",
            "minimalism",
        ],
    ),
    (
        Umbrella::Jazz,
        &[
            "jazz", "swing", "blues", "soul", "funk", "r&b", "bebop", "fusion", "lounge", "bossa",
        ],
    ),
    (
        Umbrella::Folk,
        &[
            "folk",
            "country",
            "celtic",
            "bluegrass",
            "americana",
            "singer-songwriter",
            "acoustic",
            "traditional",
        ],
    ),
    (
        Umbrella::NewAge,
        &["new age", "meditation", "relaxation", "healing"],
    ),
    (Umbrella::Rock, &["rock", "psychedelic", "progressive"]),
    (
        Umbrella::Pop,
        &[
            "pop",
            "j-pop",
            "k-pop",
            "idol",
            "disco",
            "easy listening",
            "vocal",
        ],
    ),
];

/// The folder a canonical genre files under.
pub fn umbrella_for(genre: &str) -> Umbrella {
    let lower = genre.to_lowercase();
    UMBRELLA_KEYWORDS
        .iter()
        .find(|(_, kws)| kws.iter().any(|kw| lower.contains(kw)))
        .map(|(u, _)| *u)
        .unwrap_or(Umbrella::Other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_fold_to_canonical() {
        let cases = [
            ("JPop", "J-Pop"),
            ("J-Pop", "J-Pop"),
            ("DOMESTIC(J-POPS)", "J-Pop"),
            ("Death Metal/Black Metal", "Death Metal"),
            ("Hip-Hop/Rap", "Hip Hop"),
            ("Rap", "Hip Hop"),
            ("Jungle/Drum'n'bass", "Drum and Bass"),
            ("DubStep", "Dubstep"),
            ("Video Game", "Video Game Music"),
            ("Game", "Video Game Music"),
            ("Electronica", "Electronic"),
            ("Psytrance, Goa", "Psytrance"),
            ("Prog-Rock/Art Rock", "Progressive Rock"),
            ("Books & Spoken", "Spoken Word"),
        ];
        for (raw, want) in cases {
            assert_eq!(normalize_tag(raw).as_deref(), Some(want), "{raw}");
        }
    }

    #[test]
    fn junk_is_none() {
        for raw in [
            "",
            " ",
            "145",
            "atrilli.net",
            "www.x.org",
            "Unclassifiable",
            "Other",
            "K Theory",
        ] {
            assert_eq!(normalize_tag(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn generic_tags_are_title_cased_and_collapsed() {
        assert_eq!(
            normalize_tag("  heavy   metal ").as_deref(),
            Some("Heavy Metal")
        );
        assert_eq!(normalize_tag("Metal").as_deref(), Some("Metal"));
        assert_eq!(
            normalize_tag("post-hardcore").as_deref(),
            Some("Post-Hardcore")
        );
        assert_eq!(normalize_tag("EDM").as_deref(), Some("EDM"));
        assert_eq!(normalize_tag("Prog.").as_deref(), Some("Prog."));
    }

    #[test]
    fn musicbrainz_genres_become_title_case() {
        assert_eq!(
            canonical_genre("melodic death metal"),
            "Melodic Death Metal"
        );
        assert_eq!(canonical_genre("drum and bass"), "Drum and Bass");
        assert_eq!(canonical_genre("hip hop"), "Hip Hop");
        assert_eq!(canonical_genre("rock"), "Rock");
    }

    #[test]
    fn umbrella_precedence() {
        let cases = [
            ("Symphonic Metal", Umbrella::Metal),
            ("Progressive Metal", Umbrella::Metal),
            ("Industrial Metal", Umbrella::Metal),
            ("Metalcore", Umbrella::Metal),
            ("Djent", Umbrella::Metal),
            ("Progressive Rock", Umbrella::Rock),
            ("Hard Rock", Umbrella::Rock),
            ("Classic Rock", Umbrella::Rock),
            ("Industrial", Umbrella::Electronic),
            ("Post-Hardcore", Umbrella::Alternative),
            ("Pop Punk", Umbrella::Alternative),
            ("Alternative Rock", Umbrella::Alternative),
            ("Indie Rock", Umbrella::Alternative),
            ("Video Game Music", Umbrella::Soundtrack),
            ("Anime", Umbrella::Soundtrack),
            ("Hip Hop", Umbrella::HipHop),
            ("Trance", Umbrella::Electronic),
            ("Dance", Umbrella::Electronic),
            ("Vocaloid", Umbrella::Electronic),
            ("J-Pop", Umbrella::Pop),
            ("Pop", Umbrella::Pop),
            ("Jazz", Umbrella::Jazz),
            ("Blues", Umbrella::Jazz),
            ("Classical", Umbrella::Classical),
            ("Celtic Folk", Umbrella::Folk),
            ("Country", Umbrella::Folk),
            ("New Age", Umbrella::NewAge),
            ("Happy Hardcore", Umbrella::Electronic),
            ("Hardcore Techno", Umbrella::Electronic),
            ("Hardcore", Umbrella::Alternative),
            ("Garage Rock", Umbrella::Rock),
            ("UK Garage", Umbrella::Electronic),
            ("Symphonic Rock", Umbrella::Rock),
            ("Industrial Rock", Umbrella::Rock),
            ("Dancehall", Umbrella::Other),
            ("Reggae", Umbrella::Other),
            ("Dub Techno", Umbrella::Electronic),
            ("Comedy", Umbrella::Other),
            ("Gospel", Umbrella::Other),
            ("Spoken Word", Umbrella::Other),
        ];
        for (genre, want) in cases {
            assert_eq!(umbrella_for(genre), want, "{genre}");
        }
    }

    #[test]
    fn display_names_round_trip() {
        for u in Umbrella::ALL {
            assert_eq!(Umbrella::from_display_name(u.display_name()), Some(u));
        }
        assert_eq!(Umbrella::from_display_name("nope"), None);
    }
}
