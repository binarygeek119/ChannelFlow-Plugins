//! The ready-made Binarygeek119 channel presets (v1.0.0's ChannelPresets).
//!
//! These are a quick way to stand up the original lineup. Each preset carries
//! the two dial-number styles v1.0.0 assigned (retro / modern), and its
//! channel artwork from v1.0.0's logo set — the preset page shows it and
//! "Create missing channels" uploads it as the created channel's watermark.
//! They are *not* the only way to add channels — presets only create the ones
//! that are missing, and everything here is editable or creatable on the
//! Channels tab.

/// One ready-made channel template, in both v1 numbering styles.
pub struct Preset {
    pub id: &'static str,
    /// v1.0.0 original dial number ("retro").
    pub legacy: f64,
    /// v1.0.0 subchannel dial number ("modern").
    pub subchannel: f64,
    pub name: &'static str,
    pub category: &'static str,
    pub description: &'static str,
    /// The v1.0.0 channel artwork, relative to this plugin's web root
    /// (`logos/…`); used as the preset's logo and the creation watermark.
    pub logo: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset { id: "channelflow-flashback", legacy: 119.0, subchannel: 119.1, name: "FlashBack TV", category: "TV Shows", description: "1970–2009 TV and movies (first-episode year for series)", logo: "logos/Shows/FlashBack_TV.png" },
    Preset { id: "channelflow-retro", legacy: 120.0, subchannel: 119.2, name: "Retro TV", category: "TV Shows", description: "1910–1969 TV and movies (first-episode year for series)", logo: "logos/Shows/Retro_TV.png" },
    Preset { id: "channelflow-open-swim", legacy: 121.0, subchannel: 119.3, name: "[OpenSwim]", category: "TV Shows", description: "Nick, Disney, Fox Kids, and Cartoon Network style kids TV/movies; any year; TV-PG max", logo: "logos/Shows/[open_swim].png" },
    Preset { id: "channelflow-reality", legacy: 135.0, subchannel: 124.1, name: "Flip Television", category: "TV Shows", description: "Reality TV themed shows and movies", logo: "logos/Shows/Flip_Television.png" },
    Preset { id: "channelflow-past-tense-news", legacy: 137.0, subchannel: 124.3, name: "Past Tense News", category: "News", description: "Home movies treated as live breaking news", logo: "logos/News/Past_Tense_News.png" },
    Preset { id: "channelflow-crime", legacy: 136.0, subchannel: 124.2, name: "Cops And Robbers", category: "TV Shows", description: "Crime and cop themed TV shows and movies (genre or plot)", logo: "logos/Shows/cops_and_robbers.png" },
    Preset { id: "channelflow-comedy", legacy: 138.0, subchannel: 124.4, name: "Slappy", category: "TV Shows", description: "Fox network clone: comedy TV and movies with Friday 5–8pm Slappy's Toon Takeover", logo: "logos/Shows/Slappy.png" },
    Preset { id: "channelflow-game-shows", legacy: 146.0, subchannel: 126.1, name: "Winning", category: "TV Shows", description: "Game shows channel", logo: "logos/Shows/winning.png" },
    Preset { id: "channelflow-education", legacy: 147.0, subchannel: 126.2, name: "GET LEARNEDED", category: "TV Shows", description: "Educational TV shows and movies", logo: "logos/Shows/GET_LEARNEDED.png" },
    Preset { id: "channelflow-youtube", legacy: 148.0, subchannel: 126.3, name: "YouTube TV", category: "TV Shows", description: "Content from the Jellyfin TV library curated for YouTube", logo: "logos/Shows/YouTube_TV.png" },
    Preset { id: "channelflow-creature", legacy: 203.0, subchannel: 203.1, name: "Creature Double Feature", category: "Movies", description: "Creature and monster movies and TV (genre, plot, or tags)", logo: "logos/Movies/Creature_Double_Feature.png" },
    Preset { id: "channelflow-hero", legacy: 204.0, subchannel: 203.2, name: "Hero TV", category: "Movies", description: "Anyone who saves or protects people — heroes, rescuers, and champions", logo: "logos/Movies/Hero_TV.png" },
    Preset { id: "channelflow-funny", legacy: 205.0, subchannel: 203.3, name: "That's Funny", category: "Movies", description: "Stand-up comedy movies and shows", logo: "logos/Movies/That's_Funny.png" },
    Preset { id: "channelflow-parody", legacy: 312.0, subchannel: 312.1, name: "The Parody Channel", category: "Music Videos", description: "Parody music videos", logo: "logos/Music Videos Channels/The-Parody-Channel.png" },
    Preset { id: "channelflow-rap", legacy: 313.0, subchannel: 312.2, name: "Rap On Tap", category: "Music Videos", description: "Rap and hip hop music videos", logo: "logos/Music Videos Channels/Rap-On-Tap.png" },
    Preset { id: "channelflow-music-video", legacy: 314.0, subchannel: 312.3, name: "HeadPhone Jack", category: "Music Videos", description: "All other music videos", logo: "logos/Music Videos Channels/HeadPhone_Jack.png" },
];