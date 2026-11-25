//! PICS product-info parsing and fetching.

use super::*;

/// Platform-matched non-DLC depot rows.
pub(crate) fn platform_depot_rows<'a, 'text>(
    depots: &'a steam_vdf_parser::Obj<'text>,
    platform: DepotPlatform,
) -> Vec<(
    u32,
    &'a steam_vdf_parser::Obj<'text>,
    Option<&'a steam_vdf_parser::Obj<'text>>,
)> {
    let mut rows = Vec::new();
    for (key, value) in depots.iter() {
        // Only numeric keys are depots.
        let Ok(depot_id) = key.parse::<u32>() else {
            continue;
        };
        let Some(obj) = value.as_obj() else { continue };
        // Exclude DLC content depots.
        if obj.get("dlcappid").is_some() {
            continue;
        }
        let oslist = obj
            .get("config")
            .and_then(|v| v.as_obj())
            .and_then(|c| c.get("oslist"))
            .and_then(|v| v.as_str());
        if !should_keep_depot(oslist, platform) {
            continue;
        }
        let public = obj
            .get("manifests")
            .and_then(|v| v.as_obj())
            .and_then(|m| m.get("public"))
            .and_then(|v| v.as_obj());
        rows.push((depot_id, obj, public));
    }
    rows
}

pub fn should_keep_depot(oslist: Option<&str>, target: DepotPlatform) -> bool {
    oslist.is_none_or(|os| {
        target.in_oslist(os) || !DepotPlatform::ALL.iter().any(|p| p.in_oslist(os))
    })
}

/// Platforms some depot's oslist names, in `ALL` order. `[Windows]` when none does.
pub(crate) fn available_platforms(
    depots: Option<&steam_vdf_parser::Obj<'_>>,
) -> Vec<DepotPlatform> {
    let oslists: Vec<&str> = depots
        .into_iter()
        .flat_map(|depots| depots.values())
        .filter_map(|depot| {
            depot
                .get_obj(&["config"])
                .and_then(|c| c.get("oslist"))
                .and_then(|o| o.as_str())
        })
        .collect();
    let platforms: Vec<DepotPlatform> = DepotPlatform::ALL
        .into_iter()
        .filter(|p| oslists.iter().any(|os| p.in_oslist(os)))
        .collect();
    if platforms.is_empty() {
        vec![DepotPlatform::Windows]
    } else {
        platforms
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct ProductInfoEnvelopeWrapper(pub HashMap<String, ProductInfoEnvelope>);

impl ProductInfoEnvelopeWrapper {
    pub fn into_inner(self) -> Option<ProductInfoEnvelope> {
        self.0.into_values().next()
    }
}

pub(super) fn parse_product_info_envelope(vdf: &str) -> Result<ProductInfoEnvelope> {
    if let Ok(parsed) = keyvalues_serde::from_str::<ProductInfoEnvelope>(vdf) {
        return Ok(parsed);
    }
    let wrapper: ProductInfoEnvelopeWrapper =
        keyvalues_serde::from_str(vdf).context("failed parsing product info VDF (wrapper)")?;
    wrapper
        .into_inner()
        .context("product info envelope was empty")
}

pub(super) fn parse_launch_info_from_vdf(appid: u32, raw_vdf: &str) -> Result<Vec<LaunchInfo>> {
    let parsed: ProductInfoEnvelope =
        parse_product_info_envelope(raw_vdf).context("failed to parse product info VDF")?;

    let config = parsed
        .appinfo
        .as_ref()
        .and_then(|appinfo| appinfo.config.as_ref())
        .or(parsed.config.as_ref())
        .ok_or_else(|| anyhow!("missing config section in product info for app {appid}"))?;

    if config.launch.is_empty() {
        bail!("no launch entries found for app {appid}")
    }

    let mut options = Vec::new();
    for (id, entry) in &config.launch {
        let exe = entry.executable.as_deref().unwrap_or("");
        let os_list = entry.config.as_ref().and_then(|c| c.oslist.as_deref());
        let description = entry.description.as_deref().unwrap_or("Game");

        // Pick the launch target from the entry's oslist, falling back to the
        // executable extension and finally the host OS.
        let target = if let Some(os) = os_list {
            if os.contains("linux") {
                LaunchTarget::NativeLinux
            } else if os.contains("windows") {
                LaunchTarget::WindowsProton
            } else if os.contains("macos") {
                continue; // we don't launch macOS builds
            } else {
                LaunchTarget::WindowsProton
            }
        } else if exe.ends_with(".exe") || exe.ends_with(".bat") {
            LaunchTarget::WindowsProton
        } else if exe.contains("linux") || exe.ends_with(".sh") {
            LaunchTarget::NativeLinux
        } else {
            #[cfg(target_os = "linux")]
            {
                LaunchTarget::NativeLinux
            }
            #[cfg(target_os = "windows")]
            {
                LaunchTarget::WindowsProton
            }
            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
            {
                LaunchTarget::WindowsProton
            }
        };

        options.push(LaunchInfo {
            app_id: appid,
            id: id.clone(),
            description: if description == "Game" && !exe.is_empty() {
                exe.to_string()
            } else {
                description.to_string()
            },
            executable: exe.to_string(),
            arguments: entry.arguments.clone().unwrap_or_default(),
            workingdir: entry.workingdir.clone(),
            target,
        });
    }

    if options.is_empty() {
        bail!("no suitable launch option found for app {appid}");
    }

    // Sort options: prefer key "0", then by id
    options.sort_by(|a, b| {
        if a.id == "0" {
            return std::cmp::Ordering::Less;
        }
        if b.id == "0" {
            return std::cmp::Ordering::Greater;
        }
        a.id.cmp(&b.id)
    });

    Ok(options)
}

/// Whether the buffer is text VDF.
pub(super) fn pics_buffer_is_text(buffer: &[u8]) -> bool {
    buffer
        .first()
        .map(|&b| b == 0x22 || b == 0x7B)
        .unwrap_or(false)
}

pub fn find_vdf_in_pics(buffer: &[u8]) -> Result<steam_vdf_parser::Vdf<'static>> {
    let is_text = pics_buffer_is_text(buffer);

    if is_text {
        let text = String::from_utf8_lossy(buffer);
        return steam_vdf_parser::parse_text(&text)
            .map(|v| v.into_owned())
            .map_err(|e| anyhow!("Text VDF parse error: {}", e));
    }

    if let Ok(vdf) = steam_vdf_parser::parse_binary(buffer) {
        return Ok(vdf.into_owned());
    }

    for offset in 1..std::cmp::min(128, buffer.len()) {
        if let Ok(vdf) = steam_vdf_parser::parse_binary(&buffer[offset..]) {
            tracing::info!("Success! Found VDF at offset {}", offset);
            return Ok(vdf.into_owned());
        }
    }

    bail!("Failed to locate valid VDF (Text or Binary) in PICS buffer")
}

/// PICS appinfo nests an app's sections (`common`, `extended`, `config`, `depots`)
/// under a single root key — either the literal `appinfo` or the numeric appid.
/// Return the inner value that actually holds those sections, descending one level
/// past the wrapper when present so callers can navigate by section name directly.
/// Install folder from a PICS app section.
pub(crate) fn pics_installdir(section: &steam_vdf_parser::Value) -> Option<String> {
    section
        .get_str(&["config", "installdir"])
        .or_else(|| section.get_str(&["common", "installdir"]))
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

pub(crate) fn pics_app_section<'a, 'text>(
    root: &'a steam_vdf_parser::Value<'text>,
) -> &'a steam_vdf_parser::Value<'text> {
    fn has_sections(v: &steam_vdf_parser::Value) -> bool {
        ["common", "extended", "config", "depots"]
            .iter()
            .any(|k| v.get(k).is_some())
    }
    if has_sections(root) {
        return root;
    }
    if let Some(inner) = root
        .as_obj()
        .and_then(|o| o.values().find(|v| has_sections(v)))
    {
        return inner;
    }
    root
}

/// The `depots` object, descending wrappers.
pub(crate) fn pics_depots_value<'a, 'text>(
    vdf: &'a steam_vdf_parser::Vdf<'text>,
) -> Option<&'a steam_vdf_parser::Value<'text>> {
    pics_app_section(vdf.value()).get("depots")
}

pub fn parse_pics_product_info(buffer: &[u8]) -> Result<HashMap<u64, u64>> {
    let is_text = pics_buffer_is_text(buffer);

    if is_text {
        parse_text_vdf(buffer)
    } else {
        parse_binary_vdf_with_offset(buffer)
    }
}

/// Walk a parsed PICS appinfo VDF root and collect its `public`-branch depot →
/// manifest-id map. Looks for `depots` at the root or one level under `appinfo`,
/// skips non-English-tagged depots, and resolves each manifest id via
/// [`extract_manifest_id_robust`]. Shared by the text and binary parse paths.
pub(super) fn depot_map_from_vdf_root(root: &steam_vdf_parser::Value) -> HashMap<u64, u64> {
    let mut depot_map = HashMap::new();
    let depots_val = pics_app_section(root).get("depots");

    if let Some(depots) = depots_val.and_then(|v| v.as_obj()) {
        for (key, value) in depots.iter() {
            if let Ok(depot_id) = key.parse::<u64>() {
                let lang = value
                    .get_obj(&["config"])
                    .and_then(|c| c.get("language"))
                    .and_then(|l| l.as_str());
                if let Some(lang) = lang
                    && lang != "english"
                    && !lang.is_empty()
                {
                    continue;
                }

                if let Some(m_id) = extract_manifest_id_robust(value, "public") {
                    depot_map.insert(depot_id, m_id);
                }
            }
        }
    }
    depot_map
}

pub(super) fn parse_text_vdf(data: &[u8]) -> Result<HashMap<u64, u64>> {
    let text = String::from_utf8_lossy(data);
    let mut depot_map = HashMap::new();

    if let Ok(vdf) = steam_vdf_parser::parse_text(&text) {
        depot_map = depot_map_from_vdf_root(vdf.value());
    }

    if depot_map.is_empty() {
        let mut current_depot = 0;
        let mut inside_depots = false;
        let mut inside_manifests = false;
        let mut inside_public = false;
        let mut depot_langs = HashMap::new();

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.contains("\"depots\"") {
                inside_depots = true;
                continue;
            }
            if !inside_depots {
                continue;
            }

            if trimmed == "}" {
                if inside_public {
                    inside_public = false;
                } else if inside_manifests {
                    inside_manifests = false;
                }
                continue;
            }

            if trimmed.starts_with("\"manifests\"") {
                inside_manifests = true;
                continue;
            }
            if inside_manifests && trimmed.starts_with("\"public\"") {
                inside_public = true;
                continue;
            }

            let parts = extract_quoted_values(trimmed);
            if parts.len() == 1 {
                if let Ok(id) = parts[0].parse::<u64>() {
                    current_depot = id;
                    inside_manifests = false;
                    inside_public = false;
                }
            } else if parts.len() >= 2 && current_depot > 0 {
                let key = parts[0].to_lowercase();
                if inside_public && key == "gid" {
                    if let Ok(gid) = parts[1].parse::<u64>()
                        && gid > 0
                    {
                        depot_map.insert(current_depot, gid);
                    }
                } else if key == "language" {
                    depot_langs.insert(current_depot, parts[1].to_lowercase());
                } else if !inside_manifests && (key == "manifest" || key == "gid") {
                    // Fallback for flat structure
                    if let Ok(gid) = parts[1].parse::<u64>()
                        && gid > 0
                    {
                        depot_map.insert(current_depot, gid);
                    }
                }
            }
        }

        // Apply Language Filter to manual scan results
        depot_map.retain(|id, _| {
            if let Some(lang) = depot_langs.get(id)
                && lang != "english"
                && !lang.is_empty()
            {
                return false;
            }
            true
        });
    }

    if depot_map.is_empty() {
        bail!("Text scan found no depots");
    }

    Ok(depot_map)
}

pub(super) fn parse_binary_vdf_with_offset(data: &[u8]) -> Result<HashMap<u64, u64>> {
    if let Ok(vdf) = find_vdf_in_pics(data) {
        // Ensure the located VDF is an object before walking it, preserving the
        // original error when it is not.
        vdf.as_obj().context("root is not an object")?;
        let depot_map = depot_map_from_vdf_root(vdf.value());
        if !depot_map.is_empty() {
            return Ok(depot_map);
        }
    }
    bail!("Failed to locate valid Binary VDF in PICS buffer")
}

pub(super) fn extract_manifest_id_robust(
    value: &steam_vdf_parser::Value,
    branch: &str,
) -> Option<u64> {
    if let Some(obj) = value.as_obj() {
        // Deep search for branch manifest
        if let Some(manifests) = obj.get("manifests").and_then(|v| v.as_obj())
            && let Some(branch_entry) = manifests.get(branch)
        {
            // It can be a direct string or a gid object
            if let Some(gid_str) = branch_entry.as_str()
                && let Ok(gid) = gid_str.parse::<u64>()
            {
                return Some(gid);
            }
            if let Some(gid_val) = branch_entry.as_u64() {
                return Some(gid_val);
            }
            if let Some(branch_obj) = branch_entry.as_obj()
                && let Some(gid) = branch_obj.get("gid")
            {
                if let Some(s) = gid.as_str() {
                    return s.parse().ok();
                }
                return gid.as_u64();
            }
        }

        // Direct gid
        if let Some(gid_entry) = obj.get("gid") {
            if let Some(gid_str) = gid_entry.as_str() {
                return gid_str.parse::<u64>().ok();
            }
            if let Some(gid_val) = gid_entry.as_u64() {
                return Some(gid_val);
            }
        }
    }

    None
}

#[derive(Debug, Deserialize)]
pub(super) struct ProductInfoEnvelope {
    #[serde(default)]
    appinfo: Option<ProductInfoAppInfo>,
    #[serde(default)]
    config: Option<ProductInfoConfig>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ProductInfoAppInfo {
    #[serde(default)]
    config: Option<ProductInfoConfig>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ProductInfoConfig {
    #[serde(default)]
    launch: HashMap<String, ProductLaunchEntry>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ProductLaunchEntry {
    #[serde(default)]
    pub executable: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub workingdir: Option<String>,
    #[serde(default)]
    pub config: Option<ProductLaunchConfigInner>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ProductLaunchConfigInner {
    #[serde(default)]
    oslist: Option<String>,
}

pub(super) fn split_args(args: &str) -> Vec<String> {
    args.split_whitespace().map(ToString::to_string).collect()
}

/// One-app PICS `ProductInfo` buffer request.
pub(crate) async fn pics_app_buffer(
    connection: &Connection,
    appid: u32,
    job_context: &'static str,
) -> Result<Vec<u8>> {
    let app = pics_app_info(connection, appid, None, job_context).await?;
    if !app.missing_token() {
        return Ok(app.buffer().to_vec());
    }
    // Token-gated app: fetch token, retry.
    let token = pics_access_token(connection, appid).await?;
    let app = pics_app_info(connection, appid, Some(token), job_context).await?;
    Ok(app.buffer().to_vec())
}

/// PICS buffer, fetched with access token.
pub(crate) async fn pics_app_buffer_with_token(
    connection: &Connection,
    appid: u32,
    job_context: &'static str,
) -> Result<Vec<u8>> {
    let token = pics_access_token(connection, appid).await?;
    let app = pics_app_info(connection, appid, Some(token), job_context).await?;
    Ok(app.buffer().to_vec())
}

pub(super) async fn pics_app_info(
    connection: &Connection,
    appid: u32,
    access_token: Option<u64>,
    job_context: &'static str,
) -> Result<steam_vent_proto::steammessages_clientserver_appinfo::cmsg_client_picsproduct_info_response::AppInfo>
{
    let mut request = CMsgClientPICSProductInfoRequest::new();
    request
        .apps
        .push(cmsg_client_picsproduct_info_request::AppInfo {
            appid: Some(appid),
            access_token,
            ..Default::default()
        });

    let response: CMsgClientPICSProductInfoResponse =
        connection.job(request).await.context(job_context)?;

    response
        .apps
        .into_iter()
        .find(|entry| entry.appid() == appid)
        .ok_or_else(|| anyhow!("missing appinfo payload for app {appid}"))
}

/// PICS access token for one app.
pub(super) async fn pics_access_token(connection: &Connection, appid: u32) -> Result<u64> {
    let mut request = CMsgClientPICSAccessTokenRequest::new();
    request.appids.push(appid);
    let response: CMsgClientPICSAccessTokenResponse = connection
        .job(request)
        .await
        .context("failed requesting PICS access token")?;
    if response.app_denied_tokens.contains(&appid) {
        bail!("Steam denied access to app {appid} (no license)");
    }
    response
        .app_access_tokens
        .iter()
        .find(|t| t.appid() == appid)
        .map(|t| t.access_token())
        .ok_or_else(|| anyhow!("no PICS access token returned for app {appid}"))
}
