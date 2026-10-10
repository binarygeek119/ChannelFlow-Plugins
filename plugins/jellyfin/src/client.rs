//! The Jellyfin HTTP client: server test, library list, paged item sync,
//! and image fetch. Every call goes out with `X-Emby-Token`.

use channelflow_plugin_api::media::{Connection, Library};
use channelflow_plugin_api::PluginError;
use reqwest::StatusCode;
use serde::Deserialize;

pub struct JellyfinClient {
    http: reqwest::Client,
    base: String,
    token: String,
    user_id: String,
}

/// What `test()` found, so the connection form can explain itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestVerdict {
    Ok,
    Unreachable,
    AuthFailed,
    BadUrl,
}

impl JellyfinClient {
    pub fn new(connection: &Connection, api_key: &str) -> Result<Self, PluginError> {
        // Trim both: a key or URL pasted from the Jellyfin dashboard usually
        // carries a stray space or newline, which the server then rejects.
        let base = connection.url.trim().trim_end_matches('/').to_string();
        url::Url::parse(&base).map_err(|_| PluginError::new("not a valid server URL"))?;
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(!connection.verify_tls)
            .build()
            .map_err(|error| PluginError::new(format!("building the HTTP client: {error}")))?;
        Ok(Self {
            http,
            base,
            token: api_key.trim().to_string(),
            user_id: connection.sync_user_id.clone().unwrap_or_default(),
        })
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        // Send the token both ways. Jellyfin accepts the legacy
        // `X-Emby-Token` header and the standard
        // `Authorization: MediaBrowser Token="…"`; different versions and
        // reverse proxies honor one or the other, and a valid key that only
        // one form satisfies looked like a rejected key.
        self.http
            .get(format!("{}{}", self.base, path))
            .header("X-Emby-Token", &self.token)
            .header(
                "Authorization",
                format!("MediaBrowser Token=\"{}\"", self.token),
            )
            .header("Accept", "application/json")
    }

    /// Distinguishes unreachable / bad url / auth failure so the UI can
    /// explain what is wrong. Auth tries both the header form and the
    /// `?api_key=` query form; very new Jellyfin builds and some proxies
    /// ignore the headers, and only one of the two forms is honored.
    pub async fn test(&self) -> TestVerdict {
        match self
            .http
            .get(format!("{}/System/Info/Public", self.base))
            .send()
            .await
        {
            Err(_) => return TestVerdict::Unreachable,
            Ok(response) if response.status().is_success() => {}
            Ok(_) => return TestVerdict::BadUrl,
        }
        let header_ok = self.get("/Users").send().await;
        match header_ok {
            Ok(response) if response.status().is_success() => return TestVerdict::Ok,
            Ok(response) if response.status() == StatusCode::UNAUTHORIZED => {}
            Ok(_) => return TestVerdict::BadUrl,
            Err(_) => return TestVerdict::Unreachable,
        }
        // Headers were refused — try the API key on the query string, the way
        // a browser test does.
        match self
            .http
            .get(format!("{}/Users?api_key={}", self.base, self.token))
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => TestVerdict::Ok,
            Ok(response) if response.status() == StatusCode::UNAUTHORIZED => TestVerdict::AuthFailed,
            Ok(_) => TestVerdict::BadUrl,
            Err(_) => TestVerdict::Unreachable,
        }
    }

    /// True when no API key was supplied at all, so the form can say that
    /// instead of blaming a key the user never entered.
    pub fn has_token(&self) -> bool {
        !self.token.is_empty()
    }

    pub async fn libraries(&self) -> Result<Vec<Library>, PluginError> {
        let raw: Vec<LibraryPayload> = self
            .get("/Library/VirtualFolders")
            .send()
            .await
            .map_err(|error| PluginError::new(format!("listing libraries: {error}")))?
            .error_for_status()
            .map_err(|error| PluginError::new(format!("listing libraries: {error}")))?
            .json()
            .await
            .map_err(|error| PluginError::new(format!("listing libraries: {error}")))?;
        Ok(raw
            .into_iter()
            .map(|library| Library {
                remote_id: library.item_id,
                name: library.name,
                collection_type: library.collection_type,
            })
            // Boxsets and home videos are special views, not media to put on
            // a channel lineup; keep them out of the picker and the sync.
            .filter(|library| {
                !matches!(
                    library.collection_type.as_deref(),
                    Some("boxsets") | Some("homevideos")
                )
            })
            .collect())
    }

    /// One 200-item page of a library, recursive, with the fields the sync
    /// layer and the file picker need.
    pub async fn items(&self, library_id: &str, offset: usize) -> Result<ItemPage, PluginError> {
        let fields = "Path,Overview,Genres,Studios,People,ProviderIds,MediaSources,MediaStreams,Chapters,DateCreated,ProductionYear,RunTimeTicks,CommunityRating,CriticRating,OfficialRating,Taglines,OriginalTitle,SortName,Container,PremiereDate,IndexNumber,ParentIndexNumber,SeriesName,SeriesId,AlbumId,AlbumArtist";
        let types = "Movie,Series,Episode,Audio,MusicVideo";
        // Without a sync user the bare /Items endpoint is right (an admin API
        // key). /Users//Items - the empty-user form - 404s on newer Jellyfin.
        // get() prepends the base, so only build the path here.
        let listing = if self.user_id.is_empty() {
            "/Items".to_string()
        } else {
            format!("/Users/{}/Items", self.user_id)
        };
        let url = format!(
            "{listing}?ParentId={library_id}&Recursive=true&IncludeItemTypes={types}&Fields={fields}&StartIndex={offset}&Limit=200"
        );
        let response = self
            .get(&url)
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .map_err(|error| PluginError::new(format!("fetching items: {error}")))?;
        response
            .json()
            .await
            .map_err(|error| PluginError::new(format!("fetching items: {error}")))
    }

    pub async fn image(&self, item_id: &str, kind: &str) -> Result<Vec<u8>, PluginError> {
        let bytes = self
            .get(&format!("/Items/{item_id}/Images/{kind}?maxWidth=680"))
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .map_err(|error| PluginError::new(format!("fetching {kind} image: {error}")))?
            .bytes()
            .await
            .map_err(|error| PluginError::new(format!("fetching {kind} image: {error}")))?;
        Ok(bytes.to_vec())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LibraryPayload {
    // Older Jellyfin calls the library id "ItemId"; newer builds (the 12/13
    // line) use "Id". Accept both so a newer server is not an empty catalog.
    #[serde(alias = "Id")]
    pub item_id: String,
    pub name: String,
    #[serde(default)]
    pub collection_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemPage {
    pub items: Vec<serde_json::Value>,
    pub total_record_count: usize,
}