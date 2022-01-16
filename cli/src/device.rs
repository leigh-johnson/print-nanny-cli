use anyhow::{ Result };
use clap::arg_enum;
use log:: { debug };

use printnanny_services::printnanny_api::{ ApiConfig, ApiService};
use printnanny_api_client::models;

arg_enum!{
    #[derive(PartialEq, Debug, Clone)]
    pub enum DeviceAction{
        Get,
        Setup
    }
}

pub struct DeviceCmd {
    pub action: DeviceAction,
    pub service: ApiService
}
impl DeviceCmd {
    pub async fn new(action: DeviceAction, api_config: ApiConfig, data_dir: &str) -> Result<Self> {
        let service = ApiService::new(api_config, data_dir)?;
        Ok(Self { service, action })
    }
    pub async fn handle(&self) -> Result<String>{
        let result = match self.action {
            DeviceAction::Get => self.service.device_retrieve().await?,
            DeviceAction::Setup => self.service.device_setup().await?,
        };
        debug!("Success action={} result={:?}", self.action, result);
        Ok(self.service.to_string_pretty::<models::Device>(result)?)
    }    
}