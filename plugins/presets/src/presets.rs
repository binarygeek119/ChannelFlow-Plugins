//! The ready-made Binarygeek119 channel presets (v1.0.0's ChannelPresets).
//!
//! These are a quick way to stand up the original lineup. Each preset carries
//! the two dial-number styles v1.0.0 assigned, copied from that version's
//! ChannelPresets: the **retro** whole-number position (119, 123.1, 124.2, …)
//! and the **modern** subchannel position (119.1, 124.2, 312.3, …). They are
//! *not* the only way to add channels — presets only create the ones that are
//! missing, and everything here is editable or creatable on the Channels tab.

/// One ready-made channel template, in both v1 numbering styles.
pub struct Preset {
    pub id: &'static str,
    /// v1.0.0 original dial number ("retro"): 119, 120, 123.1, 124.2, 125, …
    pub legacy: f64,
    /// v1.0.0 subchannel dial number ("modern"): 119.1, 119.2, 124.2, 312.3, …
    pub subchannel: f64,
    pub name: &'static str,
    pub category: &'static str,
    pub description: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset { id: "channelflow-flashback", legacy: 119.0, subchannel: 119.1, name: "FlashBack TV", category: "TV Shows", description: "1970–2009 TV and movies (first-episode year for series)" },
    Preset { id: "channelflow-retro", legacy: 120.0, subchannel: 119.2, name: "Retro TV", category: "TV Shows", description: "1910–1969 TV and movies (first-episode year for series)" },
    Preset { id: "channelflow-open-swim", legacy: 121.0, subchannel: 119.3, name: "[OpenSwim]", category: "TV Shows", description: "Nick, Disney, Fox Kids, and Cartoon Network style kids TV/movies; any year; TV-PG max" },
    Preset { id: "channelflow-reality", legacy: 122.0, subchannel: 119.4, name: "Flip Television", category: "TV Shows", description: "Reality TV themed shows and movies" },
    Preset { id: "channelflow-live-news", legacy: 123.1, subchannel: 119.8, name: "FlowWire News", category: "News", description: "Live RSS news channel with optional TTS" },
    Preset { id: "channelflow-weatherstar4000", legacy: 124.0, subchannel: 119.6, name: "WeatherStar4000", category: "Weather", description: "Live WeatherStar 4000+ MPEG-TS weather channel" },
    Preset { id: "channelflow-weatherstar3000", legacy: 124.2, subchannel: 119.7, name: "WeatherStar3000", category: "Weather", description: "Live WeatherStar 3000+ MPEG-TS weather channel" },
    Preset { id: "channelflow-past-tense-news", legacy: 125.0, subchannel: 124.1, name: "Past Tense News", category: "News", description: "Home movies treated as live breaking news" },
    Preset { id: "channelflow-crime", legacy: 128.0, subchannel: 124.2, name: "Cops And Robbers", category: "TV Shows", description: "Crime and cop themed TV shows and movies (genre or plot)" },
    Preset { id: "channelflow-comedy", legacy: 129.0, subchannel: 124.3, name: "Slappy", category: "TV Shows", description: "Fox network clone: comedy TV and movies with Friday 5–8pm Slappy's Toon Takeover" },
    Preset { id: "channelflow-game-shows", legacy: 130.0, subchannel: 126.1, name: "Winning", category: "TV Shows", description: "Game shows channel" },
    Preset { id: "channelflow-education", legacy: 133.0, subchannel: 126.2, name: "GET LEARNEDED", category: "TV Shows", description: "Educational TV shows and movies" },
    Preset { id: "channelflow-youtube", legacy: 134.0, subchannel: 126.3, name: "YouTube TV", category: "TV Shows", description: "Content from the Jellyfin TV library curated for YouTube" },
    Preset { id: "channelflow-creature", legacy: 203.0, subchannel: 203.1, name: "Creature Double Feature", category: "Movies", description: "Creature and monster movies and TV (genre, plot, or tags)" },
    Preset { id: "channelflow-hero", legacy: 204.0, subchannel: 203.2, name: "Hero TV", category: "Movies", description: "Anyone who saves or protects people — heroes, rescuers, and champions" },
    Preset { id: "channelflow-funny", legacy: 205.0, subchannel: 203.3, name: "That's Funny", category: "Movies", description: "Stand-up comedy movies and shows" },
    Preset { id: "channelflow-parody", legacy: 312.0, subchannel: 312.1, name: "The Parody Channel", category: "Music Videos", description: "Parody music videos" },
    Preset { id: "channelflow-rap", legacy: 313.0, subchannel: 312.2, name: "Rap On Tap", category: "Music Videos", description: "Rap and hip hop music videos" },
    Preset { id: "channelflow-music-video", legacy: 314.0, subchannel: 312.3, name: "HeadPhone Jack", category: "Music Videos", description: "All other music videos" },
];