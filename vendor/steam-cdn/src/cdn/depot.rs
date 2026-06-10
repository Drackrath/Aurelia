use keyvalues_parser::{Value, Vdf};
use std::str;

use crate::error::Error;

#[derive(Debug)]
pub struct Manifest {
    pub branch: String,
    pub gid: String,
    pub size: String,
    pub download: String,
    pub encrypted: bool,
}

impl Manifest {
    pub fn gid(&self) -> Option<u64> {
        self.gid.parse::<u64>().ok()
    }
}

#[derive(Debug)]
pub struct Branch {
    pub name: String,
    pub description: Option<String>,
    pub build_id: u32,
    pub time_updated: Option<u64>,
}

#[derive(Debug)]
pub struct Depot {
    pub depot_id: u32,
    pub manifests: Vec<Manifest>,
}

impl Depot {
    pub fn new(depot_id: u32) -> Self {
        Self {
            depot_id,
            manifests: Vec::new(),
        }
    }

    fn parse_manifests(&mut self, value: &[Value<'_>], r#type: &str) -> Result<(), Error> {
        if let Some(manifests_map) = value
            .first()
            .ok_or(Error::NoneOption)?
            .get_obj()
            .ok_or(Error::NoneOption)?
            .get(r#type)
        {
            for (key, value) in &manifests_map
                .first()
                .ok_or(Error::NoneOption)?
                .get_obj()
                .ok_or(Error::NoneOption)?
                .0
            {
                let data = value
                    .first()
                    .ok_or(Error::NoneOption)?
                    .get_obj()
                    .ok_or(Error::NoneOption)?;
                self.manifests.push(Manifest {
                    branch: key.to_string(),
                    gid: data
                        .get("gid")
                        .and_then(|v| v[0].get_str())
                        .unwrap_or_default()
                        .to_string(),
                    size: data
                        .get("size")
                        .and_then(|v| v[0].get_str())
                        .unwrap_or_default()
                        .to_string(),
                    download: data
                        .get("download")
                        .and_then(|v| v[0].get_str())
                        .unwrap_or_default()
                        .to_string(),
                    encrypted: r#type.starts_with("encrypted"),
                })
            }
        }
        Ok(())
    }

    pub fn vdf_parse(&mut self, value: &[Value<'_>]) -> Result<(), Error> {
        self.parse_manifests(value, "manifests")?;
        self.parse_manifests(value, "encryptedmanifests")?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct AppDepots {
    pub app_id: u32,
    pub app_name: Option<String>,
    pub depots: Vec<Depot>,
    pub branches: Vec<Branch>,
}

impl AppDepots {
    pub fn new(app_id: u32) -> Self {
        Self {
            app_id,
            app_name: None,
            depots: Vec::new(),
            branches: Vec::new(),
        }
    }

    pub fn vdf_parse(&mut self, buffer: &[u8]) -> Result<(), Error> {
        if let Ok(vdf) = str::from_utf8(buffer) {
            let kv = Vdf::parse(vdf)?;
            let appinfo = kv
                .value
                .get_obj()
                .ok_or(Error::Unexpected("failed to get appinfo value".to_string()))?;
            self.app_name = appinfo
                .get("common")
                .ok_or(Error::Unexpected("no appinfo.common key".to_string()))?
                .first()
                .ok_or(Error::Unexpected(
                    "no first entry of appinfo.common".to_string(),
                ))?
                .get_obj()
                .ok_or(Error::Unexpected(
                    "failed to get appinfo.common body".to_string(),
                ))?
                .get("name")
                .ok_or(Error::Unexpected("no appinfo.common.name key".to_string()))?
                .first()
                .map(|s| s.to_string());
            let depots_map = &appinfo
                .get("depots")
                .ok_or(Error::Unexpected("no depots key".to_string()))?
                .first()
                .ok_or(Error::Unexpected(
                    "no first entry of depots object".to_string(),
                ))?
                .get_obj()
                .ok_or(Error::Unexpected(
                    "failed to get depots body object".to_string(),
                ))?
                .0;
            for (key, value) in depots_map {
                if let Ok(depot_id) = key.parse::<u32>() {
                    let mut depot = Depot::new(depot_id);
                    depot.vdf_parse(value)?;
                    self.depots.push(depot);
                } else if key == "branches" {
                    // let branches_map = &value
                    //     .first()
                    //     .ok_or(Error::NoneOption)?
                    //     .get_obj()
                    //     .ok_or(Error::NoneOption)?
                    //     .0;
                    // for (key, value) in branches_map {
                    //     println!("{:?} {:?}", key, value);
                    // }
                }
            }
        }

        Ok(())
    }
}
