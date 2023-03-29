use diesel::prelude::*;
use printnanny_settings::git2::Email;
use serde::{Deserialize, Serialize};

use chrono::{DateTime, Utc};
use log::info;

use crate::connection::establish_sqlite_connection;
use crate::schema::email_alert_settings;
use crate::schema::pis;

#[derive(
    Queryable, Identifiable, Insertable, Clone, Debug, PartialEq, Default, Serialize, Deserialize,
)]
#[diesel(table_name = pis)]
pub struct Pi {
    pub id: i32,
    pub last_boot: Option<String>,
    pub hostname: String,
    pub created_dt: String,
    pub moonraker_api_url: String,
    pub mission_control_url: String,
    pub octoprint_url: String,
    pub swupdate_url: String,
    pub syncthing_url: String,
    pub preferred_dns: String,
    pub octoprint_server_id: Option<i32>,
    pub system_info_id: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, AsChangeset)]
#[diesel(table_name = pis)]
pub struct UpdatePi {
    pub last_boot: Option<String>,
    pub hostname: Option<String>,
    pub created_dt: Option<String>,
    pub moonraker_api_url: Option<String>,
    pub mission_control_url: Option<String>,
    pub octoprint_url: Option<String>,
    pub swupdate_url: Option<String>,
    pub syncthing_url: Option<String>,
    pub preferred_dns: Option<String>,
    pub octoprint_server_id: Option<i32>,
    pub system_info_id: Option<i32>,
}

impl From<printnanny_api_client::models::Pi> for UpdatePi {
    fn from(obj: printnanny_api_client::models::Pi) -> UpdatePi {
        let urls = *obj.urls;
        let preferred_dns = match obj.network_settings {
            Some(network_settings) => match network_settings.preferred_dns {
                Some(result) => result,
                None => printnanny_api_client::models::PreferredDnsType::Multicast,
            },
            None => printnanny_api_client::models::PreferredDnsType::Multicast,
        }
        .to_string();
        let octoprint_server_id = obj
            .octoprint_server
            .map(|octoprint_server| octoprint_server.id);
        let system_info_id = obj.system_info.map(|system_info| system_info.id);
        UpdatePi {
            last_boot: obj.last_boot,
            hostname: None,
            created_dt: None,
            moonraker_api_url: Some(urls.moonraker_api),
            mission_control_url: Some(urls.mission_control),
            octoprint_url: Some(urls.octoprint),
            swupdate_url: Some(urls.swupdate),
            syncthing_url: Some(urls.syncthing),
            preferred_dns: Some(preferred_dns),
            octoprint_server_id,
            system_info_id,
        }
    }
}

impl From<printnanny_api_client::models::Pi> for Pi {
    fn from(obj: printnanny_api_client::models::Pi) -> Pi {
        let urls = *obj.urls;
        let preferred_dns = match obj.network_settings {
            Some(network_settings) => match network_settings.preferred_dns {
                Some(result) => result,
                None => printnanny_api_client::models::PreferredDnsType::Multicast,
            },
            None => printnanny_api_client::models::PreferredDnsType::Multicast,
        };
        let octoprint_server_id = obj
            .octoprint_server
            .map(|octoprint_server| octoprint_server.id);

        let system_info_id = obj.system_info.map(|system_info| system_info.id);

        Pi {
            id: obj.id,
            last_boot: obj.last_boot,
            hostname: obj.hostname,
            created_dt: obj.created_dt,
            moonraker_api_url: urls.moonraker_api,
            mission_control_url: urls.mission_control,
            octoprint_url: urls.octoprint,
            swupdate_url: urls.swupdate,
            syncthing_url: urls.syncthing,
            preferred_dns: preferred_dns.to_string(),
            octoprint_server_id,
            system_info_id,
        }
    }
}

impl Pi {
    pub fn get_id(connection_str: &str) -> Result<i32, diesel::result::Error> {
        use crate::schema::pis::dsl::*;
        let connection = &mut establish_sqlite_connection(connection_str);
        let result: i32 = pis.select(id).first(connection)?;
        Ok(result)
    }
    pub fn get(connection_str: &str) -> Result<Pi, diesel::result::Error> {
        use crate::schema::pis::dsl::*;

        let connection = &mut establish_sqlite_connection(connection_str);
        let result: Pi = pis.order_by(id).first::<Pi>(connection)?;
        info!("printnanny_edge_db::cloud::Pi get {:#?}", &result);
        Ok(result)
    }
    pub fn insert(connection_str: &str, row: Pi) -> Result<(), diesel::result::Error> {
        let mut connection = establish_sqlite_connection(connection_str);
        let row = diesel::insert_into(pis::dsl::pis)
            .values(row)
            .execute(&mut connection)?;
        info!("printnanny_edge_db::cloud::Pi created {}", &row);
        Ok(())
    }
    pub fn update(
        connection_str: &str,
        pi_id: i32,
        changeset: UpdatePi,
    ) -> Result<(), diesel::result::Error> {
        let mut connection = establish_sqlite_connection(connection_str);
        let result = diesel::update(pis::table.filter(pis::id.eq(pi_id)))
            .set(changeset)
            .execute(&mut connection)?;
        info!("printnanny_edge_db::cloud::Pi with id={} updated", &result);
        Ok(())
    }
}

#[derive(
    Queryable, Identifiable, Insertable, Clone, Debug, PartialEq, Default, Serialize, Deserialize,
)]
#[diesel(table_name = email_alert_settings)]
pub struct EmailAlertSettings {
    pub id: i32,
    pub created_dt: DateTime<Utc>,
    pub updated_dt: DateTime<Utc>,
    pub progress_percent: i32,
    pub print_quality_enabled: bool,
    pub print_started_enabled: bool,
    pub print_done_enabled: bool,
    pub print_progress_enabled: bool,
    pub print_paused_enabled: bool,
    pub print_cancelled_enabled: bool,
}

#[derive(Clone, Debug, PartialEq, AsChangeset)]
#[diesel(table_name =  email_alert_settings)]
pub struct UpdateEmailAlertSettings<'a> {
    pub updated_dt: Option<&'a DateTime<Utc>>,
    pub progress_percent: Option<&'a i32>,
    pub print_quality_enabled: Option<&'a bool>,
    pub print_started_enabled: Option<&'a bool>,
    pub print_done_enabled: Option<&'a bool>,
    pub print_progress_enabled: Option<&'a bool>,
    pub print_paused_enabled: Option<&'a bool>,
    pub print_cancelled_enabled: Option<&'a bool>,
}

impl EmailAlertSettings {
    pub fn get(connection_str: &str) -> Result<EmailAlertSettings, diesel::result::Error> {
        use crate::schema::email_alert_settings::dsl::*;

        let connection = &mut establish_sqlite_connection(connection_str);
        let result: EmailAlertSettings = email_alert_settings
            .order_by(id)
            .first::<EmailAlertSettings>(connection)?;
        info!(
            "printnanny_edge_db::cloud::EmailAlertSettings row found {:#?}",
            &result
        );
        Ok(result)
    }
    pub fn insert(
        connection_str: &str,
        row: EmailAlertSettings,
    ) -> Result<(), diesel::result::Error> {
        let mut connection = establish_sqlite_connection(connection_str);
        let row = diesel::insert_into(email_alert_settings::dsl::email_alert_settings)
            .values(row)
            .execute(&mut connection)?;
        info!(
            "printnanny_edge_db::cloud::EmailAlertSettings row inserted {}",
            &row
        );
        Ok(())
    }
    pub fn update(
        connection_str: &str,
        row_id: i32,
        changeset: UpdateEmailAlertSettings,
    ) -> Result<(), diesel::result::Error> {
        let mut connection = establish_sqlite_connection(connection_str);
        let result =
            diesel::update(email_alert_settings::table.filter(email_alert_settings::id.eq(row_id)))
                .set(changeset)
                .execute(&mut connection)?;
        info!(
            "printnanny_edge_db::cloud::EmailAlertSettings with id={} updated",
            &result
        );
        Ok(())
    }
}
