use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use figment::providers::{Env, Format, Json, Serialized, Toml};
use figment::value::{Dict, Map};
use figment::{Figment, Metadata, Profile, Provider};
use glob::glob;
use lazy_static::lazy_static;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};

use crate::printnanny_api::ApiService;
use printnanny_api_client::models;

use crate::error::{PrintNannySettingsError, ServiceError};
use crate::paths::{PrintNannyPaths, DEFAULT_PRINTNANNY_SETTINGS_FILE};
use crate::settings::cam::PrintNannyCamSettings;
use crate::settings::klipper::KlipperSettings;
use crate::settings::mainsail::MainsailSettings;
use crate::settings::moonraker::MoonrakerSettings;
use crate::settings::octoprint::OctoPrintSettings;
use crate::settings::vcs::{VersionControlledSettings, VersionControlledSettingsError};
use crate::settings::SettingsFormat;
use crate::state::PrintNannyCloudData;

// FACTORY_RESET holds the struct field names of PrintNannyCloudConfig
// each member of FACTORY_RESET is written to a separate config fragment under /etc/printnanny/conf.d
// as the name implies, this const is used for performing a reset of any config data modified from defaults
const FACTORY_RESET: [&str; 2] = ["cloud", "systemd_units"];

const DEFAULT_PRINTNANNY_SETTINGS_GIT_REMOTE: &str =
    "https://github.com/bitsy-ai/printnanny-settings.git";
const DEFAULT_PRINTNANNY_SETTINGS_GIT_EMAIL: &str = "robots@printnanny.ai";
const DEFAULT_PRINTNANNY_SETTINGS_GIT_NAME: &str = "PrintNanny";

lazy_static! {
    static ref DEFAULT_SYSTEMD_UNITS: HashMap<String, SystemdUnit> = {
        let mut m = HashMap::new();

        // printnanny-vision.service
        m.insert(
            "printnanny-vision.service".to_string(),
            SystemdUnit {
                unit: "printnanny-vision.service".to_string(),
                enabled: true,
            },
        );

        // octoprint.service
        m.insert(
            "octoprint.service".to_string(),
            SystemdUnit {
                unit: "octoprint.service".to_string(),
                enabled: true,
            },
        );

        // mainsail.service
        m.insert(
            "mainsail.service".to_string(),
            SystemdUnit {
                unit: "mansail.service".to_string(),
                enabled: false,
            },
        );
        m
    };
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct NatsConfig {
    pub uri: String,
    pub require_tls: bool,
}

impl Default for NatsConfig {
    fn default() -> Self {
        Self {
            uri: "nats://localhost:4222".to_string(),
            require_tls: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrintNannyCloudProxy {
    pub hostname: String,
    pub base_path: String,
    pub url: String,
}

impl Default for PrintNannyCloudProxy {
    fn default() -> Self {
        let hostname = sys_info::hostname().unwrap_or_else(|_| "localhost".to_string());
        let base_path = "/printnanny-cloud".into();
        let url = format!("http://{}{}", hostname, base_path);
        Self {
            hostname,
            base_path,
            url,
        }
    }
}

#[derive(Debug, Clone, clap::ValueEnum, Eq, Deserialize, Serialize, PartialEq)]
pub enum VideoSrcType {
    File,
    Device,
    Uri,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SystemdUnit {
    unit: String,
    enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct GitSettings {
    pub remote: String,
    pub email: String,
    pub name: String,
    pub default_branch: String,
}

impl Default for GitSettings {
    fn default() -> Self {
        Self {
            remote: DEFAULT_PRINTNANNY_SETTINGS_GIT_REMOTE.into(),
            email: DEFAULT_PRINTNANNY_SETTINGS_GIT_EMAIL.into(),
            name: DEFAULT_PRINTNANNY_SETTINGS_GIT_NAME.into(),
            default_branch: "main".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PrintNannySettings {
    pub cam: PrintNannyCamSettings,
    pub git: GitSettings,
    pub paths: PrintNannyPaths,
    pub klipper: KlipperSettings,
    pub mainsail: MainsailSettings,
    pub moonraker: MoonrakerSettings,
    pub octoprint: OctoPrintSettings,
}

impl Default for PrintNannySettings {
    fn default() -> Self {
        let git = GitSettings::default();
        Self {
            cam: PrintNannyCamSettings::default(),
            paths: PrintNannyPaths::default(),
            klipper: KlipperSettings::default(),
            octoprint: OctoPrintSettings::default(),
            moonraker: MoonrakerSettings::default(),
            mainsail: MainsailSettings::default(),
            git,
        }
    }
}

impl PrintNannySettings {
    pub fn new() -> Result<Self, PrintNannySettingsError> {
        let figment = Self::figment()?;
        let mut result: PrintNannySettings = figment.extract()?;

        result.octoprint = OctoPrintSettings::from_dir(&result.paths.settings_dir);
        debug!("Initialized config {:?}", result);

        Ok(result)
    }
    pub fn dashboard_url(&self) -> String {
        let hostname = sys_info::hostname().unwrap_or_else(|_| "printnanny".to_string());
        format!("http://{}.local/", hostname)
    }
    pub fn find_value(key: &str) -> Result<figment::value::Value, PrintNannySettingsError> {
        let figment = Self::figment()?;
        Ok(figment.find_value(key)?)
    }

    pub async fn connect_cloud_account(
        &self,
        base_path: String,
        bearer_access_token: String,
    ) -> Result<(), ServiceError> {
        let state_file = self.paths.state_file();
        let state_lock = self.paths.state_lock();

        let mut state = PrintNannyCloudData::load(&state_file)?;
        state.api.base_path = base_path;
        state.api.bearer_access_token = Some(bearer_access_token);

        state.save(&state_file, &state_lock, true)?;

        let mut api_service = ApiService::new()?;

        // sync data models
        api_service.sync().await?;
        let mut state = PrintNannyCloudData::load(&self.paths.state_file())?;
        let pi_id = state.pi.unwrap().id;
        // download credential and device identity bundled in license.zip
        api_service.pi_download_license(pi_id).await?;
        // mark setup complete
        let req = models::PatchedPiRequest {
            setup_finished: Some(true),
            // None values are skipped by serde serializer
            sbc: None,
            hostname: None,
            fqdn: None,
            favorite: None,
        };
        api_service.pi_partial_update(pi_id, req).await?;
        let pi = api_service.pi_retrieve(pi_id).await?;
        state.pi = Some(pi);
        state.save(&self.paths.state_file(), &self.paths.state_lock(), true)?;
        Ok(())
    }

    // pub async fn sync(&self) -> Result<(), ServiceError> {
    //     let mut service = ApiService::new()?;
    //     service.sync().await
    // }

    // intended for use with Rocket's figmment
    pub fn from_figment(figment: Figment) -> Figment {
        figment.merge(Self::figment().unwrap())
    }

    // Load configuration with the following order of precedence:
    //
    // 1) Environment variables prefixed with PRINTNANNY_ (highest)
    // Example:
    //    PRINTNANNY_NATS_APP__NATS_URI="nats://localhost:4222" will override all other nats_uri settings
    //
    // 2) PRINTNANNY_SETTINGS .toml. configuration file
    //
    // 3) Glob pattern of .toml and .json configuration file fragments in conf.d folder
    //
    // 4) Defaults (from implement Default)

    pub fn check_file_from_env_var(var: &str) -> Result<(), PrintNannySettingsError> {
        // try reading env var
        match env::var(var) {
            Ok(value) => {
                // check that value exists
                let path = PathBuf::from(value);
                match path.exists() {
                    true => Ok(()),
                    false => Err(PrintNannySettingsError::ConfigFileNotFound { path }),
                }
            }
            Err(_) => {
                warn!(
                    "PRINTNANNY_SETTINGS not set. Initializing from PrintNannyCloudConfig::default()"
                );
                Ok(())
            }
        }
    }

    // load figment fragments from all *.toml and *.json files relative to base_dir
    fn load_confd(base_dir: &Path, figment: Figment) -> Result<Figment, PrintNannySettingsError> {
        let toml_glob = format!("{}/*.toml", &base_dir.display());
        let json_glob = format!("{}/*.json", &base_dir.display());

        let result = Self::read_path_glob::<Json>(&json_glob, figment);
        let result = Self::read_path_glob::<Toml>(&toml_glob, result);
        Ok(result)
    }

    pub fn figment() -> Result<Figment, PrintNannySettingsError> {
        // merge file in PRINTNANNY_SETTINGS env var (if set)
        let result = Figment::from(Self { ..Self::default() })
            .merge(Toml::file(Env::var_or(
                "PRINTNANNY_SETTINGS",
                DEFAULT_PRINTNANNY_SETTINGS_FILE,
            )))
            // allow nested environment variables:
            // PRINTNANNY_SETTINGS_KEY__SUBKEY
            .merge(Env::prefixed("PRINTNANNY_SETTINGS_").split("__"));

        // extract paths, to load application state conf.d fragments
        let lib_settings_file: String = result
            .find_value("paths.state_dir")
            .unwrap()
            .deserialize::<String>()
            .unwrap();
        let paths = PrintNannyPaths {
            state_dir: PathBuf::from(lib_settings_file),
            ..PrintNannyPaths::default()
        };
        // merge application state
        let result = Self::load_confd(&paths.lib_confd(), result)?;
        // if PRINTNANNY_SETTINGS env var is set, check file exists and is readable
        Self::check_file_from_env_var("PRINTNANNY_SETTINGS")?;

        // finally, re-merge PRINTNANNY_SETTINGS and PRINTNANNY_ENV so these values take highest precedence
        let result = result
            .merge(Toml::file(Env::var_or(
                "PRINTNANNY_SETTINGS",
                DEFAULT_PRINTNANNY_SETTINGS_FILE,
            )))
            // allow nested environment variables:
            // PRINTNANNY_KEY__SUBKEY
            .merge(Env::prefixed("PRINTNANNY_SETTINGS_").split("__"));

        info!("Finalized PrintNannyCloudConfig: \n {:?}", result);
        Ok(result)
    }

    pub fn from_toml(f: PathBuf) -> Result<Self, PrintNannySettingsError> {
        let figment = PrintNannySettings::figment()?.merge(Toml::file(f));
        Ok(figment.extract()?)
    }

    pub fn to_toml_string(&self) -> Result<String, PrintNannySettingsError> {
        let result = toml::ser::to_string_pretty(self)?;
        Ok(result)
    }

    fn read_path_glob<T: 'static + figment::providers::Format>(
        pattern: &str,
        figment: Figment,
    ) -> Figment {
        debug!("Merging config from {}", &pattern);
        let mut result = figment;
        for entry in glob(pattern).expect("Failed to read glob pattern") {
            match entry {
                Ok(path) => {
                    let key = path.file_stem().unwrap().to_str().unwrap();
                    debug!("Merging key={} config from {}", &key, &path.display());
                    result = result.clone().merge(T::file(&path));
                }
                Err(e) => error!("{:?}", e),
            }
        }
        result
    }

    pub fn try_factory_reset(&self) -> Result<(), PrintNannySettingsError> {
        // for each key/value pair in FACTORY_RESET, remove file
        for key in FACTORY_RESET.iter() {
            let filename = format!("{}.json", key);
            let filename = self.paths.lib_confd().join(filename);
            fs::remove_file(&filename)?;
            info!("Removed {} data {:?}", key, filename);
        }
        Ok(())
    }

    // Save settings to PRINTNANNY_SETTINGS (default: /var/lib/printnanny/PrintNannySettings.toml)
    pub fn try_save(&self) -> Result<(), PrintNannySettingsError> {
        let settings_file = self.paths.settings_file();
        let settings_data = toml::ser::to_string_pretty(self)?;
        fs::write(&settings_file, &settings_data)?;
        Ok(())
    }
    // Save settings to PRINTNANNY_SETTINGS (default: /var/lib/printnanny/PrintNannySettings.toml)
    pub fn save(&self) {
        self.try_save().expect("Failed to save PrintNannySettings");
    }

    // Save ::Default() to output file
    pub fn try_init(
        &self,
        filename: &str,
        format: &SettingsFormat,
    ) -> Result<(), PrintNannySettingsError> {
        let content: String = match format {
            SettingsFormat::Json => serde_json::to_string_pretty(self)?,
            SettingsFormat::Toml => toml::ser::to_string_pretty(self)?,
            _ => unimplemented!("try_init is not implemented for format: {}", format),
        };
        fs::write(&filename, content)?;
        Ok(())
    }

    /// Extract a `Config` from `provider`, panicking if extraction fails.
    ///
    /// # Panics
    ///
    /// If extraction fails, prints an error message indicating the failure and
    /// panics. For a version that doesn't panic, use [`Config::try_from()`].
    ///
    /// # Example
    pub fn from<T: Provider>(provider: T) -> Self {
        Self::try_from(provider).unwrap_or_else(|e| {
            error!("{:?}", e);
            panic!("aborting due to configuration error(s)")
        })
    }

    /// Attempts to extract a `Config` from `provider`, returning the result.
    ///
    /// # Example
    pub fn try_from<T: Provider>(provider: T) -> figment::error::Result<Self> {
        let figment = Figment::from(provider);
        let config = figment.extract::<Self>()?;
        Ok(config)
    }
}

impl Provider for PrintNannySettings {
    fn metadata(&self) -> Metadata {
        Metadata::named("PrintNannySettings")
    }

    fn data(&self) -> figment::error::Result<Map<Profile, Dict>> {
        let map: Map<Profile, Dict> = Serialized::defaults(self).data()?;
        Ok(map)
    }
}

#[async_trait]
impl VersionControlledSettings for PrintNannySettings {
    type SettingsModel = PrintNannySettings;
    fn from_dir(settings_dir: &Path) -> Self {
        let settings_file = settings_dir.join("printnanny/printnanny.toml");
        let result = PrintNannySettings::from_toml(settings_file).unwrap();
        result
    }
    fn get_settings_format(&self) -> SettingsFormat {
        SettingsFormat::Toml
    }
    fn get_settings_file(&self) -> PathBuf {
        self.paths
            .settings_dir
            .join("printnanny/printnanny.toml")
            .into()
    }

    async fn pre_save(&self) -> Result<(), VersionControlledSettingsError> {
        debug!("Running PrintNannySettings pre_save hook");
        Ok(())
    }

    async fn post_save(&self) -> Result<(), VersionControlledSettingsError> {
        debug!("Running PrintNannySettings post_save hook");
        Ok(())
    }
    fn validate(&self) -> Result<(), VersionControlledSettingsError> {
        todo!("OctoPrintSettings validate hook is not yet implemented");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::PRINTNANNY_SETTINGS_FILENAME;

    #[test_log::test]
    fn test_config_file_not_found() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("PRINTNANNY_SETTINGS", PRINTNANNY_SETTINGS_FILENAME);
            let result = PrintNannySettings::figment();
            assert!(result.is_err());
            Ok(())
        });
    }

    #[test_log::test]
    fn test_nested_env_var() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                PRINTNANNY_SETTINGS_FILENAME,
                r#"
                [paths]
                settings_dir = "/this/etc/path/gets/overridden"
                "#,
            )?;
            jail.set_env("PRINTNANNY_SETTINGS", PRINTNANNY_SETTINGS_FILENAME);
            let expected = PathBuf::from("testing");
            jail.set_env(
                "PRINTNANNY_SETTINGS_PATHS__SETTINGS_DIR",
                &expected.display(),
            );
            let figment = PrintNannySettings::figment().unwrap();
            let config: PrintNannySettings = figment.extract()?;
            assert_eq!(config.paths.settings_dir, expected);
            Ok(())
        });
    }

    #[test_log::test]
    fn test_paths() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                PRINTNANNY_SETTINGS_FILENAME,
                r#"
                [paths]
                settings_dir = "/opt/printnanny/"
                state_dir = "/var/lib/custom"
        
                
                [api]
                base_path = "https://print-nanny.com"
                "#,
            )?;
            jail.set_env("PRINTNANNY_SETTINGS", PRINTNANNY_SETTINGS_FILENAME);
            let figment = PrintNannySettings::figment().unwrap();
            let config: PrintNannySettings = figment.extract()?;
            assert_eq!(config.paths.data(), PathBuf::from("/var/lib/custom/data"));
            assert_eq!(config.paths.user_confd(), PathBuf::from("/opt/printnanny/"));

            Ok(())
        });
    }
    #[test_log::test]
    fn test_env_merged() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                PRINTNANNY_SETTINGS_FILENAME,
                r#"
                [paths]
                install = "/opt/printnanny/default"
                data = "/opt/printnanny/default/data"

                "#,
            )?;
            jail.set_env("PRINTNANNY_SETTINGS", PRINTNANNY_SETTINGS_FILENAME);
            let settings = PrintNannySettings::new().unwrap();
            assert_eq!(
                settings.octoprint.enabled,
                OctoPrintSettings::default().enabled,
            );
            jail.set_env("PRINTNANNY_SETTINGS_OCTOPRINT__ENABLED", "false");
            let figment = PrintNannySettings::figment().unwrap();
            let settings: PrintNannySettings = figment.extract()?;
            assert_eq!(settings.octoprint.enabled, false);
            Ok(())
        });
    }

    #[test_log::test]
    fn test_custom_conf_values() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "Local.toml",
                r#"
                [paths]
                settings_dir = ".tmp/"
                
                [octoprint]
                enabled = false
                "#,
            )?;
            jail.set_env("PRINTNANNY_SETTINGS", "Local.toml");

            let figment = PrintNannySettings::figment().unwrap();
            let settings: PrintNannySettings = figment.extract()?;

            assert_eq!(settings.paths.settings_dir, PathBuf::from(".tmp/"));
            assert_eq!(settings.octoprint.enabled, false);

            Ok(())
        });
    }

    #[test_log::test]
    fn test_save() {
        figment::Jail::expect_with(|jail| {
            let output = jail.directory().to_str().unwrap();
            jail.create_file(
                "Local.toml",
                &format!(
                    r#"
                profile = "local"

                [paths]
                state_dir = "{}"

                [octoprint]
                enabled = false
                "#,
                    output
                ),
            )?;
            jail.set_env("PRINTNANNY_SETTINGS", "Local.toml");

            let figment = PrintNannySettings::figment().unwrap();
            let mut settings: PrintNannySettings = figment.extract()?;
            fs::create_dir(settings.paths.lib_confd()).unwrap();

            settings.octoprint.enabled = true;
            settings.save();
            let figment = PrintNannySettings::figment().unwrap();
            let settings: PrintNannySettings = figment.extract()?;
            assert_eq!(settings.octoprint.enabled, true);
            Ok(())
        });
    }

    #[test_log::test]
    fn test_find_value() {
        figment::Jail::expect_with(|jail| {
            let output = jail.directory().to_str().unwrap();
            let expected: Option<String> = Some(format!("{output}/printnanny.d"));

            jail.create_file(
                "Local.toml",
                &format!(
                    r#"
                [paths]
                settings_dir = "{output}/printnanny.d"
                log_dir = "{output}/log"

                [octoprint]
                enabled = false
                "#,
                    output = &output
                ),
            )?;
            jail.set_env("PRINTNANNY_SETTINGS", "Local.toml");

            let value: Option<String> = PrintNannySettings::find_value("paths.settings_dir")
                .unwrap()
                .into_string();
            assert_eq!(value, expected);
            Ok(())
        });
    }

    #[test_log::test]
    fn test_os_release() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                PRINTNANNY_SETTINGS_FILENAME,
                r#"
                [octoprint]
                enabled = false
                "#,
            )?;
            jail.create_file(
                "os-release",
                r#"
ID=printnanny
ID_LIKE="BitsyLinux"
BUILD_ID="2022-06-18T18:46:49Z"
NAME="PrintNanny Linux"
VERSION="0.1.2 (Amber)"
VERSION_ID=0.1.2
PRETTY_NAME="PrintNanny Linux 0.1.2 (Amber)"
DISTRO_CODENAME="Amber"
HOME_URL="https://printnanny.ai"
BUG_REPORT_URL="https://github.com/bitsy-ai/printnanny-os/issues"
YOCTO_VERSION="4.0.1"
YOCTO_CODENAME="Kirkstone"
SDK_VERSION="0.1.2"
VARIANT="PrintNanny OctoPrint Edition"
VARIANT_ID=printnanny-octoprint
                "#,
            )?;
            jail.set_env("PRINTNANNY_SETTINGS", PRINTNANNY_SETTINGS_FILENAME);
            jail.set_env(
                "PRINTNANNY_SETTINGS_PATHS__OS_RELEASE",
                format!("{:?}", jail.directory().join("os-release")),
            );

            let config = PrintNannySettings::new().unwrap();
            let os_release = config.paths.load_os_release().unwrap();
            assert_eq!("2022-06-18T18:46:49Z".to_string(), os_release.build_id);
            Ok(())
        });
    }

    #[test_log::test]
    fn test_user_provided_toml_file() {
        figment::Jail::expect_with(|jail| {
            let output = jail.directory().to_str().unwrap();

            let filename = "custom.toml";

            jail.create_file(
                filename,
                &format!(
                    r#"
                profile = "local"
                [paths]
                settings_dir = "{output}/printnanny.d"
                log_dir = "{output}/log"
                "#,
                    output = output
                ),
            )?;

            let config =
                PrintNannySettings::from_toml(PathBuf::from(output).join(filename)).unwrap();
            assert_eq!(
                config.paths.settings_dir,
                PathBuf::from(format!("{}/printnanny.d", output))
            );

            Ok(())
        });
    }
}