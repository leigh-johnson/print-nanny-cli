use printnanny_services::error::ServiceError;
use printnanny_services::printnanny_api::ApiService;
use printnanny_settings::cloud::PrintNannyCloudData;
use printnanny_settings::SettingsFormat;
use std::io::{self, Write};

pub struct CloudDataCommand;

impl CloudDataCommand {
    pub async fn handle(sub_m: &clap::ArgMatches) -> Result<(), ServiceError> {
        let config: PrintNannyCloudData = PrintNannyCloudData::new()?;
        match sub_m.subcommand() {
            Some(("sync", _args)) => {
                let mut service = ApiService::new()?;
                service.sync().await?;
            }
            Some(("show", args)) => {
                let f: SettingsFormat = args.value_of_t("format").unwrap();
                let v = match f {
                    SettingsFormat::Json => serde_json::to_vec_pretty(&config)?,
                    SettingsFormat::Toml => toml::ser::to_vec(&config)?,
                    SettingsFormat::Ini | SettingsFormat::Yaml => todo!(),
                };
                io::stdout().write_all(&v)?;
            }
            _ => panic!("Expected get|sync|show subcommand"),
        };
        Ok(())
    }
}
