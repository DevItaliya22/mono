// SPDX-License-Identifier: MPL-2.0

pub mod s3;

#[cfg(test)]
mod tests;

use reqwest::blocking::{Body, Client};
use reqwest::header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap};
use s3::{CanonicalRequest, S3Credentials};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::cell::OnceCell;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const API: &str = "https://api.cloudflare.com/client/v4";
pub const BUCKET: &str = "koru-updates";
pub const HOST: &str = "updates.koru.do";
/// The same bucket, under a name that suits published media rather than app
/// updates. Q-V78 settled that a second class of asset gets a second custom
/// domain on the one origin, not a second bucket: the objects, the uploader and
/// the credentials all stay single.
pub const ASSETS_HOST: &str = "assets.koru.do";
const R2_ACCESS_KEY_ENV: &str = "CLOUDFLARE_R2_ACCESS_KEY_ID";
const R2_SECRET_KEY_ENV: &str = "CLOUDFLARE_R2_SECRET_ACCESS_KEY";

/// What a person types, and what R2 cannot serve on its own.
///
/// An R2 custom domain does **not** resolve a directory index: verified against
/// the live origin, `/releases/0.0.2-beta.1/` answers 404 while
/// `/releases/0.0.2-beta.1/index.html` answers 200. So the canonical URL only
/// works because of the transform rule below.
const INDEX_PATH: &str = "/releases/";
const INDEX_DOCUMENT: &str = "/releases/index.html";
/// The no-slash form. It would otherwise 404, and a link in a message is as
/// likely to drop the slash as keep it.
const INDEX_BARE: &str = "/releases";

const REWRITE_RULE: &str = "Koru releases index document";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CloudConfig {
    pub account_id: String,
    pub zone_id: String,
}

pub struct Cloud {
    client: Client,
    token: String,
    pub config: CloudConfig,
    /// Preloaded from the explicit R2 S3 pair when configured, or derived from
    /// an R2 API token the first time a large object needs multipart upload.
    /// The fallback costs a round trip to `/user/tokens/verify`.
    s3: OnceCell<S3Credentials>,
}

#[derive(Deserialize)]
struct Envelope<T> {
    success: bool,
    result: T,
    #[serde(default)]
    errors: Vec<ApiError>,
}

#[derive(Deserialize)]
struct ApiError {
    #[serde(default)]
    code: u64,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct Zone {
    id: String,
    account: Account,
}

#[derive(Deserialize)]
struct Account {
    id: String,
}

fn configured_s3_credentials() -> Result<OnceCell<S3Credentials>, String> {
    let access = std::env::var(R2_ACCESS_KEY_ENV).ok();
    let secret = std::env::var(R2_SECRET_KEY_ENV).ok();
    let cell = OnceCell::new();
    if let Some(credentials) = S3Credentials::from_explicit_pair(access, secret)? {
        let _ = cell.set(credentials);
    }
    Ok(cell)
}

impl Cloud {
    pub fn discover(token: String) -> Result<Self, String> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| error.to_string())?;
        let url = format!("{API}/zones?name=koru.do&status=active");
        let response: Envelope<Vec<Zone>> = client
            .get(url)
            .bearer_auth(&token)
            .send()
            .map_err(|error| error.to_string())?
            .json()
            .map_err(|error| format!("Cloudflare zone response was invalid: {error}"))?;
        if !response.success {
            return Err(api_errors(&response.errors));
        }
        let zone = response
            .result
            .into_iter()
            .next()
            .ok_or("the token cannot see an active koru.do zone")?;
        let s3 = configured_s3_credentials()?;
        Ok(Self {
            client,
            token,
            config: CloudConfig {
                account_id: zone.account.id,
                zone_id: zone.id,
            },
            s3,
        })
    }

    /// A client that can only read, and needs no credentials to do it.
    ///
    /// Everything the release index is built from — the build counter, the
    /// channel manifests, the per-build history — is served publicly from
    /// `updates.koru.do`, and [`Cloud::fetch_public`] goes through that host
    /// rather than the API. So rehearsing the page with `--dry-run` requires no
    /// Cloudflare token, which is the difference between a check anyone in the
    /// tree can run and one only the person holding the release key can.
    ///
    /// **Read-only by construction**: the account and zone are blank, so any
    /// call that would reach the API builds a nonsense path and fails rather
    /// than writing somewhere unintended. Nothing on the dry-run path touches
    /// one.
    pub fn public_reader() -> Result<Self, String> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(90))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            client,
            token: String::new(),
            config: CloudConfig {
                account_id: String::new(),
                zone_id: String::new(),
            },
            s3: OnceCell::new(),
        })
    }

    pub fn from_config(token: String, config: CloudConfig) -> Result<Self, String> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(90))
            .build()
            .map_err(|error| error.to_string())?;
        let s3 = configured_s3_credentials()?;
        Ok(Self {
            client,
            token,
            config,
            s3,
        })
    }

    pub fn setup(&self) -> Result<(), String> {
        let buckets: Value =
            self.get(&format!("/accounts/{}/r2/buckets", self.config.account_id))?;
        let exists = buckets
            .get("buckets")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item.get("name").and_then(Value::as_str) == Some(BUCKET))
            });
        if !exists {
            let _: Value = self.post(
                &format!("/accounts/{}/r2/buckets", self.config.account_id),
                &json!({ "name": BUCKET }),
            )?;
        }

        for host in [HOST, ASSETS_HOST] {
            self.attach_custom_domain(host)?;
        }
        let _: Value = self.put(
            &format!(
                "/accounts/{}/r2/buckets/{BUCKET}/domains/managed",
                self.config.account_id
            ),
            &json!({ "enabled": false }),
        )?;
        self.install_cache_rules()
    }

    /// Point one hostname at the bucket, creating the domain or updating it.
    ///
    /// Re-running is the normal case — `cloud-setup` is not a one-shot — so an
    /// existing domain is updated in place rather than treated as an error.
    fn attach_custom_domain(&self, host: &str) -> Result<(), String> {
        // The list endpoint answers `result: { "domains": [...] }`, not a bare
        // array — decoding it as one fails on a 200 with no useful message.
        let listed: Value = self.get(&format!(
            "/accounts/{}/r2/buckets/{BUCKET}/domains/custom",
            self.config.account_id
        ))?;
        let domains = listed
            .get("domains")
            .or(Some(&listed))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let domain_body = json!({
                "domain": host,
                "enabled": true,
                "zoneId": self.config.zone_id,
                "minTLS": "1.2"
        });
        if domains
            .iter()
            .any(|domain| domain.get("domain").and_then(Value::as_str) == Some(host))
        {
            let _: Value = self.put(
                &format!(
                    "/accounts/{}/r2/buckets/{BUCKET}/domains/custom/{host}",
                    self.config.account_id
                ),
                &domain_body,
            )?;
        } else {
            let _: Value = self.post(
                &format!(
                    "/accounts/{}/r2/buckets/{BUCKET}/domains/custom",
                    self.config.account_id
                ),
                &domain_body,
            )?;
        }
        Ok(())
    }

    /// **The order of these rules is not what makes them correct.**
    ///
    /// `set_cache_settings` is a *non-terminating* action, and Cloudflare's
    /// ruleset engine says of those: "the last change made by rules in the same
    /// phase will win (later rules can overwrite changes done by previous
    /// rules)". So the intuition that a more specific rule placed first wins is
    /// backwards here, and getting it backwards is expensive rather than
    /// untidy: the release index would inherit the one-year **browser** TTL
    /// from the immutable rule, and a browser TTL is not reachable by a cache
    /// purge — every visitor who loaded the page once would be stuck with that
    /// copy for a year.
    ///
    /// The fix is not to reorder, which would leave correctness resting on a
    /// subtlety the next edit breaks in silence. The immutable rule is
    /// **narrowed** so it cannot match the index at all, and the index rule is
    /// appended last as well. The result is right under either reading.
    ///
    /// `http_request_transform` runs before this phase, so these expressions
    /// see the rewritten path — which is why both spellings are excluded.
    fn install_cache_rules(&self) -> Result<(), String> {
        let phase = format!(
            "/zones/{}/rulesets/phases/http_request_cache_settings/entrypoint",
            self.config.zone_id
        );
        let koru_rules = vec![
            json!({
                "description": "Koru immutable releases",
                "expression": format!("(http.host eq \"{HOST}\" and starts_with(http.request.uri.path, \"/releases/\") and not (http.request.uri.path in {{\"{INDEX_PATH}\" \"{INDEX_DOCUMENT}\"}}))"),
                "action": "set_cache_settings",
                "action_parameters": {
                    "cache": true,
                    "edge_ttl": { "mode": "override_origin", "default": 31536000 },
                    "browser_ttl": { "mode": "override_origin", "default": 31536000 }
                },
                "enabled": true
            }),
            json!({
                "description": "Koru channel manifests",
                "expression": format!("(http.host eq \"{HOST}\" and starts_with(http.request.uri.path, \"/channels/\"))"),
                "action": "set_cache_settings",
                "action_parameters": {
                    "cache": true,
                    "edge_ttl": { "mode": "override_origin", "default": 30 },
                    "browser_ttl": { "mode": "override_origin", "default": 0 }
                },
                "enabled": true
            }),
            // Last, so it wins under the engine's real precedence as well as
            // being unreachable by the narrowed rule above.
            json!({
                "description": "Koru release index",
                "expression": format!("(http.host eq \"{HOST}\" and http.request.uri.path in {{\"{INDEX_PATH}\" \"{INDEX_DOCUMENT}\"}})"),
                "action": "set_cache_settings",
                "action_parameters": {
                    "cache": true,
                    "edge_ttl": { "mode": "override_origin", "default": 30 },
                    "browser_ttl": { "mode": "override_origin", "default": 0 }
                },
                "enabled": true
            }),
            // The precedence argument above does not reach this one: it matches
            // a different host, so no ordering against the three release rules
            // can change what either of them does. The one-year **browser** TTL
            // is only safe because the key carries the digest — publishing
            // changed bytes writes a new path, and no cache has to be told.
            json!({
                "description": "Koru media assets",
                "expression": format!("(http.host eq \"{ASSETS_HOST}\" and starts_with(http.request.uri.path, \"/videos/\"))"),
                "action": "set_cache_settings",
                "action_parameters": {
                    "cache": true,
                    "edge_ttl": { "mode": "override_origin", "default": 31536000 },
                    "browser_ttl": { "mode": "override_origin", "default": 31536000 }
                },
                "enabled": true
            }),
        ];
        let existing = self.raw_get(&phase)?;
        if existing.status().as_u16() == 404 {
            let _: Value = self.post(
                &format!("/zones/{}/rulesets", self.config.zone_id),
                &json!({
                    "name": "Koru update caching",
                    "kind": "zone",
                    "phase": "http_request_cache_settings",
                    "rules": koru_rules
                }),
            )?;
        } else {
            let envelope: Envelope<Value> = existing
                .json()
                .map_err(|error| format!("Cloudflare cache-rule response was invalid: {error}"))?;
            if !envelope.success {
                return Err(api_errors(&envelope.errors));
            }
            let id = envelope
                .result
                .get("id")
                .and_then(Value::as_str)
                .ok_or("Cloudflare did not return the cache ruleset id")?;
            let mut rules = envelope
                .result
                .get("rules")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            // Every description this function writes has to be listed, or a
            // second `cloud-setup` appends a duplicate instead of replacing.
            rules.retain(|rule| {
                !matches!(
                    rule.get("description").and_then(Value::as_str),
                    Some(
                        "Koru immutable releases"
                            | "Koru channel manifests"
                            | "Koru release index"
                            | "Koru media assets"
                    )
                )
            });
            rules.extend(koru_rules);
            let _: Value = self.put(
                &format!("/zones/{}/rulesets/{id}", self.config.zone_id),
                &json!({
                    "name": "Koru update caching",
                    "kind": "zone",
                    "phase": "http_request_cache_settings",
                    "rules": rules
                }),
            )?;
        }
        Ok(())
    }

    /// Make `/releases/` serve `/releases/index.html`.
    ///
    /// **Deliberately not called from [`Cloud::setup`].** It is the one step
    /// whose failure is survivable — the token that has provisioned this origin
    /// since the first release carries R2 Storage Write, Zone Read and Cache
    /// Rules Edit, and none of those is Transform Rules Edit. Setup's other
    /// nine steps must not be thrown away because of it, so the keep-going
    /// decision and the instructions for doing it by hand live in `main.rs`
    /// where they are beside the rest of the output.
    ///
    /// A rewrite rather than a redirect: the address bar keeps the URL the
    /// person typed, and the immutable release pages are unaffected either way.
    pub fn install_url_rewrite(&self) -> Result<(), String> {
        let phase = format!(
            "/zones/{}/rulesets/phases/http_request_transform/entrypoint",
            self.config.zone_id
        );
        let koru_rules = vec![json!({
            "description": REWRITE_RULE,
            "expression": format!("(http.host eq \"{HOST}\" and http.request.uri.path in {{\"{INDEX_PATH}\" \"{INDEX_BARE}\"}})"),
            "action": "rewrite",
            "action_parameters": { "uri": { "path": { "value": INDEX_DOCUMENT } } },
            "enabled": true
        })];
        let existing = self.raw_get(&phase)?;
        if existing.status().as_u16() == 404 {
            let _: Value = self.post(
                &format!("/zones/{}/rulesets", self.config.zone_id),
                &json!({
                    "name": "Koru URL rewrites",
                    "kind": "zone",
                    "phase": "http_request_transform",
                    "rules": koru_rules
                }),
            )?;
            return Ok(());
        }
        let envelope: Envelope<Value> = existing
            .json()
            .map_err(|error| format!("Cloudflare transform-rule response was invalid: {error}"))?;
        if !envelope.success {
            return Err(api_errors(&envelope.errors));
        }
        let id = envelope
            .result
            .get("id")
            .and_then(Value::as_str)
            .ok_or("Cloudflare did not return the transform ruleset id")?;
        let mut rules = envelope
            .result
            .get("rules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        rules.retain(|rule| rule.get("description").and_then(Value::as_str) != Some(REWRITE_RULE));
        rules.extend(koru_rules);
        let _: Value = self.put(
            &format!("/zones/{}/rulesets/{id}", self.config.zone_id),
            &json!({
                "name": "Koru URL rewrites",
                "kind": "zone",
                "phase": "http_request_transform",
                "rules": rules
            }),
        )?;
        Ok(())
    }

    /// What to add by hand when the token cannot. Printed rather than
    /// described, so it can be pasted.
    pub fn rewrite_rule_by_hand() -> String {
        format!(
            "  Rules -> Transform Rules -> URL Rewrite -> Create rule\n    \
             Name:       {REWRITE_RULE}\n    \
             Expression: (http.host eq \"{HOST}\" and http.request.uri.path in \
             {{\"{INDEX_PATH}\" \"{INDEX_BARE}\"}})\n    \
             Path:       rewrite to the static value {INDEX_DOCUMENT}"
        )
    }

    /// Reads what is *in the bucket*, which is not what the canonical URL
    /// answers. `/releases/` carries a one-year edge TTL, Cloudflare ignores a
    /// client's `no-cache` by default, and a 404 caches like anything else — so
    /// the existence check would cache its own miss and verification would then
    /// read that miss back and declare a successful upload lost. The cache key
    /// includes the query, so a nonce is what actually reaches R2.
    pub fn fetch_public(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let response = self
            .client
            .get(format!("https://{HOST}/{key}?cache-bust={nonce}"))
            .header(CACHE_CONTROL, "no-cache")
            .send()
            .map_err(|error| error.to_string())?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        let status = response.status();
        if !status.is_success() {
            return Err(format!("GET {key} returned {status}"));
        }
        Ok(Some(
            response
                .bytes()
                .map_err(|error| error.to_string())?
                .to_vec(),
        ))
    }

    /// **Is this object there, and how big is it** — without downloading it.
    ///
    /// [`Cloud::fetch_public`] reads the whole body, which is the right shape
    /// for a manifest and completely wrong for a 475 MB model archive: asking
    /// whether an upload can be skipped must not cost the download it is
    /// skipping. The cache-busting nonce is the same one and for the same
    /// reason — a 404 caches like anything else, and a cached miss would make
    /// the publisher re-upload what is already there for the length of the TTL.
    ///
    /// A missing `Content-Length` answers `Some(None)`: the object exists and
    /// its size is unknown, which is a different fact from *it is not there*
    /// and must not be collapsed into it. Read the raw header rather than
    /// `Response::content_length()`: reqwest documents that method as the
    /// response *body* length, and a HEAD response has an empty body even when
    /// its `Content-Length` header describes the selected object.
    pub fn head_public(&self, key: &str) -> Result<Option<Option<u64>>, String> {
        self.head_public_on(HOST, key)
    }

    /// The same probe against whichever hostname serves the object. Both custom
    /// domains front one bucket, so either answers; asking on the host the
    /// caller publishes to keeps its output and its check talking about the
    /// same URL.
    pub fn head_public_on(&self, host: &str, key: &str) -> Result<Option<Option<u64>>, String> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let response = self
            .client
            .head(format!("https://{host}/{key}?cache-bust={nonce}"))
            .header(CACHE_CONTROL, "no-cache")
            .send()
            .map_err(|error| error.to_string())?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        let status = response.status();
        if !status.is_success() {
            return Err(format!("HEAD {key} returned {status}"));
        }
        Ok(Some(content_length_header(response.headers())?))
    }

    /// What the *edge* says about an object, which is not what was uploaded
    /// with it — a cache rule overrides the origin's header, and that override
    /// is the whole point of the rules in `install_cache_rules`.
    ///
    /// Used to catch one specific mistake immediately rather than in a year:
    /// publishing the release index before `cloud-setup` has narrowed the
    /// one-year `/releases/` rule leaves the page with a browser TTL that no
    /// purge can reach. Reading the header back is the only way to know, since
    /// nothing about the upload itself looks wrong.
    pub fn edge_cache_control(&self, key: &str) -> Result<Option<String>, String> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let response = self
            .client
            .get(format!("https://{HOST}/{key}?probe={nonce}"))
            .send()
            .map_err(|error| error.to_string())?;
        Ok(response
            .headers()
            .get(CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string))
    }

    pub fn put_bytes(
        &self,
        key: &str,
        bytes: Vec<u8>,
        content_type: &str,
        cache_control: &str,
    ) -> Result<(), String> {
        self.put_object(key, Body::from(bytes), content_type, cache_control)
    }

    pub fn put_file(
        &self,
        key: &str,
        path: &Path,
        content_type: &str,
        cache_control: &str,
    ) -> Result<(), String> {
        let file = File::open(path).map_err(|error| error.to_string())?;
        let size = file.metadata().map_err(|error| error.to_string())?.len();
        // **The REST object endpoint refuses a body this big**, and it does so
        // as a Cloudflare error page rather than as anything a caller would
        // recognise. Every existing caller is far under it; the check is here
        // for the one that will not be, and it names the route that works.
        if size > s3::REST_UPLOAD_MAX_BYTES {
            return Err(format!(
                "{key} is {size} bytes and the Cloudflare REST object endpoint takes at most \
                 {}. Use Cloud::put_file_any_size, which routes it through the S3 \
                 multipart API instead.",
                s3::REST_UPLOAD_MAX_BYTES
            ));
        }
        self.put_object(key, Body::sized(file, size), content_type, cache_control)
    }

    /// The object endpoint takes the raw bytes as the body. A `multipart/form-data`
    /// upload is refused with 501 and code 10028 — R2 does not implement it — and
    /// the object's own metadata travels as request headers rather than parts.
    fn put_object(
        &self,
        key: &str,
        body: Body,
        content_type: &str,
        cache_control: &str,
    ) -> Result<(), String> {
        let url = format!(
            "{API}/accounts/{}/r2/buckets/{BUCKET}/objects/{key}",
            self.config.account_id
        );
        let response = self
            .client
            .put(url)
            .bearer_auth(&self.token)
            .header(CONTENT_TYPE, content_type)
            .header(CACHE_CONTROL, cache_control)
            .body(body)
            .send()
            .map_err(|error| error.to_string())?;
        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            Err(format!("upload {key} failed ({status}): {body}"))
        }
    }

    /// **Publish an object of any size, choosing the route by its length.**
    ///
    /// Everything Koru published before speech models fits the REST endpoint,
    /// so that stays the path for anything small: it needs no S3 credential, no
    /// signing and no three-step protocol. Anything approaching
    /// [`s3::REST_UPLOAD_MAX_BYTES`] goes multipart instead, because a 475 MB
    /// model archive cannot be uploaded through the REST API at all — see
    /// [`s3`]'s module note for the finding and the credential story.
    ///
    /// The chosen route is announced, because which one ran is the single most
    /// useful line to have when a publish fails.
    pub fn put_file_any_size(
        &self,
        key: &str,
        path: &Path,
        content_type: &str,
        cache_control: &str,
    ) -> Result<(), String> {
        let size = std::fs::metadata(path)
            .map_err(|error| format!("{}: {error}", path.display()))?
            .len();
        if size <= s3::MULTIPART_ABOVE_BYTES {
            println!("    uploading {key} ({size} bytes, one PUT)");
            return self.put_file(key, path, content_type, cache_control);
        }
        let plan = s3::parts(size, s3::PART_SIZE);
        println!(
            "    uploading {key} ({size} bytes, {} parts of up to {} bytes)",
            plan.len(),
            s3::PART_SIZE
        );
        self.put_file_multipart(key, path, size, content_type, cache_control)
    }

    /// The explicit R2 S3 pair, or the documented R2-token derivation fallback.
    ///
    /// Explicit credentials are installed when this client is constructed. If
    /// absent, `/user/tokens/verify` supplies the token id and the token value
    /// is hashed according to Cloudflare's documented derivation. That fallback
    /// is valid for an R2 API token, not an arbitrary custom REST token.
    fn s3_credentials(&self) -> Result<&S3Credentials, String> {
        if let Some(credentials) = self.s3.get() {
            return Ok(credentials);
        }
        let verified: Value = self.get("/user/tokens/verify")?;
        let id = verified
            .get("id")
            .and_then(Value::as_str)
            .ok_or("Cloudflare did not name this token's id, so no S3 credential can be derived")?;
        let _ = self.s3.set(S3Credentials::derive(id, &self.token));
        self.s3
            .get()
            .ok_or_else(|| "the S3 credential could not be cached".to_string())
    }

    /// `<account>.r2.cloudflarestorage.com`, which is neither the API host nor
    /// the public one. Three hosts are in play in this file and mixing them up
    /// produces a 404 that reads as a missing object.
    fn s3_host(&self) -> String {
        format!("{}.r2.cloudflarestorage.com", self.config.account_id)
    }

    /// Send one signed S3 request. `body` is already in memory because it is
    /// also what is hashed: SigV4 over a streamed body would mean either
    /// reading it twice or sending `UNSIGNED-PAYLOAD`, and a part is 64 MiB.
    fn s3_send(
        &self,
        method: reqwest::Method,
        key: &str,
        query: &[(&str, &str)],
        extra_headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Result<reqwest::blocking::Response, String> {
        let host = self.s3_host();
        let path = format!("/{BUCKET}/{key}");
        let now = chrono::Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date_stamp = now.format("%Y%m%d").to_string();
        let payload = s3::sha256_hex(&body);

        let mut headers: Vec<(String, String)> = vec![
            ("host".to_string(), host.clone()),
            ("x-amz-content-sha256".to_string(), payload.clone()),
            ("x-amz-date".to_string(), amz_date.clone()),
        ];
        for (name, value) in extra_headers {
            headers.push((name.to_ascii_lowercase(), (*value).to_string()));
        }
        let authorization = {
            let request = CanonicalRequest {
                method: method.as_str(),
                path: &path,
                query,
                headers: &headers,
                payload_sha256: &payload,
            };
            s3::authorization(self.s3_credentials()?, &amz_date, &date_stamp, &request)
        };

        let mut builder = self
            .client
            .request(method, format!("https://{host}{path}"))
            .header("authorization", authorization);
        for (name, value) in &headers {
            // `host` is reqwest's to set from the URL, and setting it here as
            // well is how a request ends up carrying two of them while the
            // signature covers one.
            if name != "host" {
                builder = builder.header(name.as_str(), value.as_str());
            }
        }
        if !query.is_empty() {
            let pairs: Vec<(&str, &str)> = query.to_vec();
            builder = builder.query(&pairs);
        }
        let response = builder
            .body(body)
            .send()
            .map_err(|error| error.to_string())?;
        if response.status().is_success() {
            Ok(response)
        } else {
            let status = response.status();
            let text = response.text().unwrap_or_default();
            if text.contains("<Code>SignatureDoesNotMatch</Code>") {
                Err(format!(
                    "S3 {path} failed ({status}): R2 rejected the credential pair. Set both {R2_ACCESS_KEY_ENV} and {R2_SECRET_KEY_ENV} to the Access Key ID and Secret Access Key shown when creating an R2 API token. Cloudflare response: {text}"
                ))
            } else {
                Err(format!("S3 {path} failed ({status}): {text}"))
            }
        }
    }

    /// Create, upload every part, complete — and abort if any of it fails.
    ///
    /// **The abort is not tidiness.** An incomplete multipart upload keeps its
    /// parts in the bucket and is billed for them, and they are not listed
    /// beside the objects, so a failed publish that left them behind would be
    /// storage nobody could find. It is best-effort on the failure path,
    /// because the original error is the one worth reporting.
    fn put_file_multipart(
        &self,
        key: &str,
        path: &Path,
        size: u64,
        content_type: &str,
        cache_control: &str,
    ) -> Result<(), String> {
        let created = self.s3_send(
            reqwest::Method::POST,
            key,
            &[("uploads", "")],
            &[
                ("content-type", content_type),
                ("cache-control", cache_control),
            ],
            Vec::new(),
        )?;
        let body = created.text().map_err(|error| error.to_string())?;
        let upload_id = s3::xml_tag(&body, "UploadId")
            .ok_or_else(|| format!("R2 did not return an UploadId for {key}: {body}"))?;

        match self.upload_every_part(key, path, size, &upload_id) {
            Ok(etags) => self.complete_multipart(key, &upload_id, &etags),
            Err(error) => {
                let _ = self.s3_send(
                    reqwest::Method::DELETE,
                    key,
                    &[("uploadId", upload_id.as_str())],
                    &[],
                    Vec::new(),
                );
                Err(error)
            }
        }
    }

    fn upload_every_part(
        &self,
        key: &str,
        path: &Path,
        size: u64,
        upload_id: &str,
    ) -> Result<Vec<(u32, String)>, String> {
        let mut file = File::open(path).map_err(|error| error.to_string())?;
        let plan = s3::parts(size, s3::PART_SIZE);
        let total = plan.len();
        let mut etags = Vec::with_capacity(total);
        for (number, offset, length) in plan {
            file.seek(SeekFrom::Start(offset))
                .map_err(|error| error.to_string())?;
            let mut buffer = vec![0u8; length as usize];
            file.read_exact(&mut buffer)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            println!("      part {number}/{total} ({length} bytes)");
            let response = self.s3_send(
                reqwest::Method::PUT,
                key,
                &[
                    ("partNumber", number.to_string().as_str()),
                    ("uploadId", upload_id),
                ],
                &[],
                buffer,
            )?;
            let etag = response
                .headers()
                .get(reqwest::header::ETAG)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| format!("R2 returned no ETag for part {number} of {key}"))?
                .to_string();
            etags.push((number, etag));
        }
        Ok(etags)
    }

    /// The completion body, which is the one piece of XML this tool writes.
    ///
    /// The ETag is echoed **verbatim, quotes included**. R2 answers it quoted
    /// and compares what it is sent against what it stored; stripping the
    /// quotes to make the XML look tidier is an `InvalidPart` at the last step
    /// of a multi-gigabyte upload.
    fn complete_multipart(
        &self,
        key: &str,
        upload_id: &str,
        etags: &[(u32, String)],
    ) -> Result<(), String> {
        let mut body = String::from("<CompleteMultipartUpload>");
        for (number, etag) in etags {
            body.push_str(&format!(
                "<Part><PartNumber>{number}</PartNumber><ETag>{etag}</ETag></Part>"
            ));
        }
        body.push_str("</CompleteMultipartUpload>");
        let response = self.s3_send(
            reqwest::Method::POST,
            key,
            &[("uploadId", upload_id)],
            &[("content-type", "application/xml")],
            body.into_bytes(),
        )?;
        // **A 200 is not success here.** S3 streams whitespace while it
        // assembles the object and can then report a failure inside a 200 body
        // — the one place in this file where the status code is not the answer.
        let text = response.text().unwrap_or_default();
        if text.contains("<Error>") {
            return Err(format!("completing {key} failed: {text}"));
        }
        Ok(())
    }

    fn raw_get(&self, path: &str) -> Result<reqwest::blocking::Response, String> {
        self.client
            .get(format!("{API}{path}"))
            .bearer_auth(&self.token)
            .send()
            .map_err(|error| error.to_string())
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let response = self.raw_get(path)?;
        decode(response)
    }

    fn post<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T, String> {
        decode(
            self.client
                .post(format!("{API}{path}"))
                .bearer_auth(&self.token)
                .header(CONTENT_TYPE, "application/json")
                .json(body)
                .send()
                .map_err(|error| error.to_string())?,
        )
    }

    fn put<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T, String> {
        decode(
            self.client
                .put(format!("{API}{path}"))
                .bearer_auth(&self.token)
                .header(CONTENT_TYPE, "application/json")
                .json(body)
                .send()
                .map_err(|error| error.to_string())?,
        )
    }
}

fn decode<T: DeserializeOwned>(response: reqwest::blocking::Response) -> Result<T, String> {
    let status = response.status();
    let envelope: Envelope<T> = response
        .json()
        .map_err(|error| format!("Cloudflare returned invalid JSON ({status}): {error}"))?;
    if status.is_success() && envelope.success {
        Ok(envelope.result)
    } else {
        Err(api_errors(&envelope.errors))
    }
}

fn content_length_header(headers: &HeaderMap) -> Result<Option<u64>, String> {
    let Some(value) = headers.get(CONTENT_LENGTH) else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|error| format!("Content-Length is not valid ASCII: {error}"))?;
    value
        .parse::<u64>()
        .map(Some)
        .map_err(|error| format!("Content-Length `{value}` is not an unsigned byte count: {error}"))
}

fn api_errors(errors: &[ApiError]) -> String {
    if errors.is_empty() {
        return "Cloudflare API request failed".into();
    }
    errors
        .iter()
        .map(|error| format!("{}: {}", error.code, error.message))
        .collect::<Vec<_>>()
        .join("; ")
}
