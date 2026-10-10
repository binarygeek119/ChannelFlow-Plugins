//! The ready-made Binarygeek119 channel presets (v1.0.0's ChannelPresets).
//!
//! These are a quick way to stand up the original lineup: one channel per
//! preset with the original whole-number dial position. They are *not* the
//! only way to add channels — presets only create the ones that are missing,
//! and everything here is editable or creatable on the Channels tab too.

/// One ready-made channel template.
pub struct Preset {
    pub id: &'static str,
    pub number: u32,
    pub name: &'static str,
    pub category: &'static str,
    pub description: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset { id: "channelflow-flashback", number: 119, name: "FlashBack TV", category: "TV Shows", description: "1970–2009 TV and movies (first-episode year for series)" },
    Preset { id: "channelflow-retro", number: 120, name: "Retro TV", category: "TV Shows", description: "1910–1969 TV and movies (first-episode year for series)" },
    Preset { id: "channelflow-open-swim", number: 121, name: "[OpenSwim]", category: "TV Shows", description: "Nick, Disney, Fox Kids, and Cartoon Network style kids TV/movies; any year; TV-PG max" },
    Preset { id: "channelflow-reality", number: 122, name: "Flip Television", category: "TV Shows", description: "Reality TV themed shows and movies" },
    Preset { id: "channelflow-live-news", number: 123, name: "FlowWire News", category: "News", description: "Live RSS news channel with optional TTS" },
    Preset { id: "channelflow-weatherstar4000", number: 124, name: "WeatherStar4000", category: "Weather", description: "Live WeatherStar 4000+ MPEG-TS weather channel" },
    Preset { id: "channelflow-weatherstar3000", number: 125, name: "WeatherStar3000", category: "Weather", description: "Live WeatherStar 3000+ MPEG-TS weather channel" },
    Preset { id: "channelflow-past-tense-news", number: 126, name: "Past Tense News", category: "News", description: "Home movies treated as live breaking news" },
    Preset { id: "channelflow-crime", number: 128, name: "Cops And Robbers", category: "TV Shows", description: "Crime and cop themed TV shows and movies (genre or plot)" },
    Preset { id: "channelflow-comedy", number: 129, name: "Slappy", category: "TV Shows", description: "Fox network clone: comedy TV and movies with Friday 5–8pm Slappy's Toon Takeover" },
    Preset { id: "channelflow-game-shows", number: 130, name: "Winning", category: "TV Shows", description: "Game shows channel" },
    Preset { id: "channelflow-education", number: 133, name: "GET LEARNEDED", category: "TV Shows", description: "Educational TV shows and movies" },
    Preset { id: "channelflow-youtube", number: 134, name: "YouTube TV", category: "TV Shows", description: "Content from the Jellyfin TV library curated for YouTube" },
    Preset { id: "channelflow-creature", number: 203, name: "Creature Double Feature", category: "Movies", description: "Creature and monster movies and TV (genre, plot, or tags)" },
    Preset { id: "channelflow-hero", number: 204, name: "Hero TV", category: "Movies", description: "Anyone who saves or protects people — heroes, rescuers, and champions" },
    Preset { id: "channelflow-funny", number: 205, name: "That's Funny", category: "Movies", description: "Stand-up comedy movies and shows" },
    Preset { id: "channelflow-holiday", number: 207, name: "The Holiday Channel", category: "Movies", description: "Seasonal holiday TV and movies" },
    Preset { id: "channelflow-parody", number: 312, name: "The Parody Channel", category: "Music Videos", description: "Parody music videos" },
    Preset { id: "channelflow-rap", number: 313, name: "Rap On Tap", category: "Music Videos", description: "Rap and hip hop music videos" },
    Preset { id: "channelflow-music-video", number: 314, name: "HeadPhone Jack", category: "Music Videos", description: "All other music videos" },
];