use log::{debug, info, warn};
use std::collections::HashMap;
use std::convert::TryInto;
use std::fs::{read_to_string, File};
use std::future::Future;
use std::io::BufReader;
use std::path::Path;

use printnanny_api_client::apis::accounts_api;
use printnanny_api_client::apis::configuration::Configuration as ReqwestConfig;
use printnanny_api_client::apis::devices_api;
use printnanny_api_client::apis::octoprint_api;
use printnanny_api_client::models;

use super::config::PrintNannyConfig;
use super::cpuinfo::RpiCpuInfo;
use super::error::{PrintNannyConfigError, ServiceError};
use super::file::open;
use super::octoprint::OctoPrintConfig;

#[derive(Debug, Clone)]
pub struct ApiService {
    pub reqwest: ReqwestConfig,
    pub config: PrintNannyConfig,
    pub pi: Option<models::Pi>,
    pub user: Option<models::User>,
}

pub fn read_model_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, std::io::Error> {
    let file = open(path)?;
    let reader = BufReader::new(file);
    let result: T = serde_json::from_reader(reader)?;
    Ok(result)
}

pub fn save_model_json<T: serde::Serialize>(model: &T, path: &Path) -> Result<(), std::io::Error> {
    serde_json::to_writer(&File::create(path)?, model)?;
    Ok(())
}

impl ApiService {
    // config priority:
    // args >> api_config.json >> anonymous api usage only
    pub fn new(config: PrintNannyConfig) -> Result<ApiService, ServiceError> {
        debug!("Initializing ApiService from config: {:?}", config);
        let reqwest = ReqwestConfig {
            base_path: config.api.base_path.to_string(),
            bearer_access_token: config.api.bearer_access_token.clone(),
            ..ReqwestConfig::default()
        };
        Ok(Self {
            reqwest,
            config,
            pi: None,
            user: None,
        })
    }
    // // alert settings API
    // pub async fn alert_settings_get_or_create(
    //     &self,
    // ) -> Result<models::AlertSettings, ServiceError> {
    //     let res = alert_settings_api::alert_settings_get_or_create_retrieve(&self.reqwest).await?;
    //     Ok(res)
    // }
    pub async fn auth_user_retreive(&self) -> Result<models::User, ServiceError> {
        Ok(accounts_api::accounts_user_retrieve(&self.reqwest).await?)
    }

    pub async fn auth_email_create(
        &self,
        email: String,
    ) -> Result<models::DetailResponse, ServiceError> {
        let req = models::EmailAuthRequest { email };
        Ok(accounts_api::accounts2fa_auth_email_create(&self.reqwest, req).await?)
    }
    pub async fn auth_token_validate(
        &self,
        email: &str,
        token: &str,
    ) -> Result<models::TokenResponse, ServiceError> {
        let req = models::CallbackTokenAuthRequest {
            email: Some(email.to_string()),
            token: token.to_string(),
            mobile: None,
        };
        Ok(accounts_api::accounts2fa_auth_token_create(&self.reqwest, req).await?)
    }

    // syncs Raspberry Pi data with PrintNanny Cloud
    // performs any necessary one-time setup tasks, like registering Cloudiot Device
    pub async fn sync(&mut self) -> Result<(), ServiceError> {
        // verify pi is authenticated
        match &self.config.pi {
            Some(pi) => {
                // always update SystemInfo
                info!("Calling device_system_info_update_or_create()");
                let system_info = self.system_info_update_or_create(pi.id).await?;
                info!("Success! Updated SystemInfo {:?}", system_info);

                // detect edition from os-release
                // performs any necessary one-time setup tasks, like registering OctoPrint Server
                let os_release = self.config.paths.load_os_release()?;
                let octoprint = match OctoPrintConfig::required(&os_release.variant_id) {
                    true => {
                        let mut octoprint = OctoPrintConfig::default();
                        let octoprint_server = self
                            .octoprint_server_update_or_create(&octoprint, pi.id)
                            .await?;
                        octoprint.server = Some(octoprint_server);
                        info!("Success! Updated OctoPrintConfig {:?}", &octoprint);
                        Some(octoprint)
                    }
                    false => {
                        debug!(
                            "OctoPrintConfig is not required for {}",
                            &os_release.variant_id
                        );
                        None
                    }
                };

                self.config.octoprint = octoprint;
                let pi = self.pi_retrieve(pi.id).await?;
                self.config.pi = Some(pi);
                self.config.try_save()?;
                Ok(())
            }
            None => Err(ServiceError::SetupIncomplete {
                field: "pi".to_string(),
                detail: None,
            }),
        }
    }

    pub async fn pi_retrieve(&self, pi_id: i32) -> Result<models::Pi, ServiceError> {
        let res = devices_api::pis_retrieve(&self.reqwest, pi_id).await?;
        Ok(res)
    }

    // pub async fn device_public_key_update_or_create(
    //     &self,
    //     device: i32,
    // ) -> Result<models::PublicKey, ServiceError> {
    //     let keyfile = &self.config.keys.ec_public_key_file();
    //     info!("Reading public key from {:?}", &keyfile);
    //     let pem = read_to_string(&keyfile)?;
    //     let req = models::PublicKeyRequest {
    //         fingerprint: self.config.keys.read_fingerprint()?,
    //         pem,
    //         device,
    //         cipher: self.config.mqtt.cipher.clone(),
    //         length: 256,
    //     };
    //     let res = devices_api::public_key_update_or_create(&self.reqwest, device, req).await?;
    //     Ok(res)
    // }

    // async fn janus_cloud_stream_get_or_create(
    //     &self,
    //     device: i32,
    // ) -> Result<models::JanusCloudStream, ServiceError> {
    //     let req = models::JanusCloudStreamRequest {
    //         device,
    //         pin: None,
    //         info: None,
    //         active: None,
    //         secret: None,
    //     };
    //     let res =
    //         janus_api::devices_janus_cloud_stream_get_or_create(&self.reqwest, device, req).await?;
    //     Ok(res)
    // }

    // async fn janus_edge_stream_get_or_create(
    //     &self,
    //     device: i32,
    //     user: i32,
    // ) -> Result<models::JanusEdgeStream, ServiceError> {
    //     let mut req: models::JanusEdgeStreamRequest = match &self.config.janus_edge_request {
    //         Some(r) => Ok(r.clone()),
    //         None => Err(ServiceError::SetupIncomplete {
    //             firstboot_file: self.config.paths.firstboot.clone(),
    //             field: "janus_edge_request".into(),
    //             detail: None,
    //         }),
    //     }?;
    //     // device and user ids are set to zero in the config rendered during firstboot
    //     // set these to correct user/device id
    //     // https://github.com/bitsy-ai/ansible-collection-printnanny/blob/main/roles/install/templates/config.toml.j2#L22
    //     req.device = device;
    //     req.auth.user = user;
    //     let res =
    //         janus_api::devices_janus_edge_stream_get_or_create(&self.reqwest, device, req).await?;
    //     Ok(res)
    // }

    async fn system_info_update_or_create(
        &self,
        pi: i32,
    ) -> Result<models::SystemInfo, ServiceError> {
        let machine_id: String = read_to_string("/etc/machine-id")?;

        // hacky parsing of rpi-specific /proc/cpuinfo
        let rpi_cpuinfo = RpiCpuInfo::new()?;
        let model = rpi_cpuinfo.model.unwrap_or_else(|| "unknown".to_string());
        let serial = rpi_cpuinfo.serial.unwrap_or_else(|| "unknown".to_string());
        let revision = rpi_cpuinfo
            .revision
            .unwrap_or_else(|| "unknown".to_string());

        let cpuinfo = procfs::CpuInfo::new()?;
        let cores: i32 = cpuinfo.num_cores().try_into().unwrap();

        let meminfo = procfs::Meminfo::new()?;
        let ram = meminfo.mem_total.try_into().unwrap();

        let os_release = self.config.paths.load_os_release()?;
        let os_release_json: HashMap<String, serde_json::Value> =
            serde_json::from_str(&serde_json::to_string(&os_release)?)?;
        let request = models::SystemInfoRequest {
            machine_id,
            serial,
            revision,
            model,
            cores,
            ram,
            pi,
            os_build_id: os_release.build_id,
            os_variant_id: os_release.variant_id,
            os_version_id: os_release.version_id,
            os_release_json: Some(os_release_json),
        };
        info!("device_system_info_update_or_create request {:?}", request);
        let res = devices_api::system_info_update_or_create(&self.reqwest, pi, request).await?;
        Ok(res)
    }

    pub async fn octoprint_server_update_or_create(
        &self,
        octoprint_config: &OctoPrintConfig,
        pi: i32,
    ) -> Result<models::OctoPrintServer, ServiceError> {
        let pip_packages = octoprint_config.pip_packages()?;
        let octoprint_version = octoprint_config.octoprint_version(&pip_packages)?.into();
        let pip_version = octoprint_config.pip_version()?;
        let printnanny_plugin_version =
            octoprint_config.printnanny_plugin_version(&pip_packages)?;
        let python_version = octoprint_config.python_version()?;
        let req = models::OctoPrintServerRequest {
            octoprint_version,
            pip_version,
            printnanny_plugin_version,
            python_version,
            pi,
        };
        debug!(
            "Sending request {:?} to octoprint_server_update_or_create",
            req
        );
        let res = octoprint_api::octoprint_server_update_or_create(&self.reqwest, req).await?;
        Ok(res)
    }

    // read <models::<T>>.json from disk cache @ /var/run/printnanny
    // hydrate cache if not found using fallback fn f (must return a Future)
    pub async fn load_model<T: serde::de::DeserializeOwned + serde::Serialize + std::fmt::Debug>(
        &self,
        path: &Path,
        f: impl Future<Output = Result<T, PrintNannyConfigError>>,
    ) -> Result<T, PrintNannyConfigError> {
        let m = read_model_json::<T>(path);
        match m {
            Ok(v) => Ok(v),
            Err(_e) => {
                warn!(
                    "Failed to read {:?} - falling back to load remote model",
                    path
                );
                let res = f.await;
                match res {
                    Ok(v) => {
                        match save_model_json::<T>(&v, path) {
                            Ok(()) => Ok(()),
                            Err(error) => Err(PrintNannyConfigError::WriteIOError {
                                path: path.to_path_buf(),
                                error,
                            }),
                        }?;
                        info!("Saved model {:?} to {:?}", &v, path);
                        Ok(v)
                    }
                    Err(e) => Err(e),
                }
            }
        }
    }
}
