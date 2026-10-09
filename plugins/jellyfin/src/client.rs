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
        let base = connection.url.trim_end_matches('/').to_string();
        url::Url::parse(&base).map_err(|_| PluginError::new("not a valid server URL"))?;
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(!connection.verify_tls)
            .build()
            .map_err(|error| PluginError::new(format!("building the HTTP client: {error}")))?;
        Ok(Self {
            http,
            base,
            token: api_key.to_string(),
            user_id: connection.sync_user_id.clone().unwrap_or_default(),
        })
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.http
            .get(format!("{}{}", self.base, path))
            .header("X-Emby-Token", &self.token)
            .header("Accept", "application/json")
    }

    /// Distinguishes unreachable / bad url / auth failure so the UI can
    /// explain what is wrong.
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
        match self.get("/Users").send().await {
            Ok(response) if response.status().is_success() => TestVerdict::Ok,
            Ok(response) if response.status() == StatusCode::UNAUTHORIZED => TestVerdict::AuthFailed,
            Ok(_) => TestVerdict::BadUrl,
            Err(_) => TestVerdict::Unreachable,
        }
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
            .collect())
    }

    /// One 200-item page of a library, recursive, with the fields the sync
    /// layer and the file picker need.
    pub async fn items(&self, library_id: &str, offset: usize) -> Result<ItemPage, PluginError> {
        let fields = "Path,Overview,Genres,Studios,People,ProviderIds,MediaSources,MediaStreams,Chapters,DateCreated,ProductionYear,RunTimeTicks,CommunityRating,CriticRating,OfficialRating,Taglines,OriginalTitle,SortName,Container,PremiereDate,IndexNumber,ParentIndexNumber,SeriesName,SeriesId,AlbumId,AlbumArtist";
        let types = "Movie,Series,Episode,Audio,MusicVideo";
        let url = format!(
            "/Users/{}/Items?ParentId={}&Recursive=true&IncludeItemTypes={}&Fields={}&StartIndex={}&Limit=200",
            self.user_id, library_id, types, fields, offset
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
    pub item_id: String,
    pub name: String,
    pub collection_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemPage {
    pub items: Vec<serde_json::Value>,
    pub total_record_count: usize,
}