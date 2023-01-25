use std::fmt::Debug;
use std::time::SystemTime;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::prelude::*;
use log::{error, info, warn};
use printnanny_settings::cam::CameraVideoSource;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::fs;

use printnanny_dbus::printnanny_asyncapi_models;
use printnanny_dbus::printnanny_asyncapi_models::{
    CameraRecordingLoadReply, CameraRecordingStarted, CameraRecordingStopped, CamerasLoadReply,
    CrashReportOsLogsReply, CrashReportOsLogsRequest, DeviceInfoLoadReply,
    PrintNannyCloudAuthReply, PrintNannyCloudAuthRequest, PrintNannyCloudSyncReply, SettingsApp,
    SettingsFile, SettingsFileApplyReply, SettingsFileApplyRequest, SettingsFileLoadReply,
    SettingsFileRevertReply, SettingsFileRevertRequest, SystemdManagerDisableUnitsReply,
    SystemdManagerEnableUnitsReply, SystemdManagerGetUnitFileStateReply,
    SystemdManagerGetUnitReply, SystemdManagerGetUnitRequest, SystemdManagerRestartUnitReply,
    SystemdManagerRestartUnitRequest, SystemdManagerStartUnitReply, SystemdManagerStartUnitRequest,
    SystemdManagerStopUnitReply, SystemdManagerStopUnitRequest, SystemdManagerUnitFilesRequest,
    SystemdUnitChange, SystemdUnitChangeState, SystemdUnitFileState, VideoStreamSettings,
};
use printnanny_dbus::systemd1::models::PRINTNANNY_RECORDING_SERVICE_TEMPLATE;

use printnanny_dbus::zbus;
use printnanny_dbus::zbus_systemd;

use printnanny_settings::git2;
use printnanny_settings::printnanny::PrintNannySettings;
use printnanny_settings::vcs::VersionControlledSettings;

use printnanny_services::printnanny_api::ApiService;

use printnanny_gst_pipelines::factory::PrintNannyPipelineFactory;

#[async_trait]
pub trait NatsRequestHandler {
    type Request: Serialize + DeserializeOwned + Clone + Debug + NatsRequestHandler;
    type Reply: Serialize + DeserializeOwned + Clone + Debug;

    fn replace_subject_pattern(subject: &str, pattern: &str, replace: &str) -> String {
        subject.replace(pattern, replace)
    }
    fn deserialize_payload(subject_pattern: &str, payload: &Bytes) -> Result<Self::Request>;
    async fn handle(&self) -> Result<Self::Reply>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "subject_pattern")]
pub enum NatsRequest {
    // pi.{pi_id}.command.camera.recording.load
    #[serde(rename = "pi.{pi_id}.command.camera.recording.load")]
    CameraRecordingLoadRequest,

    // pi.{pi_id}.command.camera.recording.start
    #[serde(rename = "pi.{pi_id}.command.camera.recording.start")]
    CameraRecordingStartRequest,

    // pi.{pi_id}.command.camera.recording.stop
    #[serde(rename = "pi.{pi_id}.command.camera.recording.stop")]
    CameraRecordingStopRequest,

    // pi.{pi_id}.cameras.load
    #[serde(rename = "pi.{pi_id}.cameras.load")]
    CameraLoadRequest,

    #[serde(rename = "pi.{pi_id}.command.cloud.sync")]
    PrintNannyCloudSyncRequest,

    // pi.{pi_id}.crash_reports.os
    #[serde(rename = "pi.{pi_id}.crash_reports.os")]
    CrashReportOsLogsRequest(CrashReportOsLogsRequest),

    // pi.{pi_id}.device_info.load
    #[serde(rename = "pi.{pi_id}.device_info.load")]
    DeviceInfoLoadRequest,

    // pi.{pi_id}.settings.*
    #[serde(rename = "pi.{pi_id}.settings.printnanny.cloud.auth")]
    PrintNannyCloudAuthRequest(PrintNannyCloudAuthRequest),
    #[serde(rename = "pi.{pi_id}.settings.file.load")]
    SettingsFileLoadRequest,
    #[serde(rename = "pi.{pi_id}.settings.file.apply")]
    SettingsFileApplyRequest(SettingsFileApplyRequest),
    #[serde(rename = "pi.{pi_id}.settings.file.revert")]
    SettingsFileRevertRequest(SettingsFileRevertRequest),

    #[serde(rename = "pi.{pi_id}.settings.camera.apply")]
    CameraSettingsFileApplyRequest(VideoStreamSettings),
    #[serde(rename = "pi.{pi_id}.settings.camera.load")]
    CameraSettingsFileLoadRequest,

    // pi.{pi_id}.dbus.org.freedesktop.systemd1.*
    #[serde(rename = "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.DisableUnit")]
    SystemdManagerDisableUnitsRequest(SystemdManagerUnitFilesRequest),
    #[serde(rename = "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.EnableUnit")]
    SystemdManagerEnableUnitsRequest(SystemdManagerUnitFilesRequest),
    #[serde(rename = "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.GetUnit")]
    SystemdManagerGetUnitRequest(SystemdManagerGetUnitRequest),
    #[serde(rename = "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.GetUnitFileState")]
    SystemdManagerGetUnitFileStateRequest(SystemdManagerGetUnitRequest),
    // TODO: : Job type reload is not applicable for unit octoprint.service.
    // #[serde(rename = "pi.dbus.org.freedesktop.systemd1.Manager.ReloadUnit")]
    // SystemdManagerReloadUnitRequest(SystemdManagerReloadUnitRequest),
    #[serde(rename = "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.RestartUnit")]
    SystemdManagerRestartUnitRequest(SystemdManagerRestartUnitRequest),
    #[serde(rename = "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.StartUnit")]
    SystemdManagerStartUnitRequest(SystemdManagerStartUnitRequest),
    #[serde(rename = "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.StopUnit")]
    SystemdManagerStopUnitRequest(SystemdManagerStopUnitRequest),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "subject_pattern")]
pub enum NatsReply {
    // pi.{pi_id}.command.camera.recording.load
    #[serde(rename = "pi.{pi_id}.command.camera.recording.load")]
    CameraRecordingLoadReply(CameraRecordingLoadReply),

    // pi.{pi_id}.command.camera.recording.start
    #[serde(rename = "pi.{pi_id}.command.camera.recording.start")]
    CameraRecordingStartReply(CameraRecordingStarted),

    // pi.{pi_id}.command.camera.recording.stop
    #[serde(rename = "pi.{pi_id}.command.camera.recording.stop")]
    CameraRecordingStopReply(CameraRecordingStopped),

    // pi.{pi_id}.cameras.load
    #[serde(rename = "pi.{pi_id}.cameras.load")]
    CameraLoadReply(CamerasLoadReply),

    #[serde(rename = "pi.{pi_id}.command.cloud.sync")]
    PrintNannyCloudSyncReply(PrintNannyCloudSyncReply),

    // pi.{pi_id}.crash_reports.os
    #[serde(rename = "pi.{pi_id}.crash_reports.os")]
    CrashReportOsLogsReply(CrashReportOsLogsReply),

    // pi.{pi_id}.device_info.load
    #[serde(rename = "pi.{pi_id}.device_info.load")]
    DeviceInfoLoadReply(DeviceInfoLoadReply),

    // pi.{pi_id}.settings.*
    #[serde(rename = "pi.{pi_id}.settings.printnanny.cloud.auth")]
    PrintNannyCloudAuthReply(PrintNannyCloudAuthReply),
    #[serde(rename = "pi.{pi_id}.settings.printnanny.load")]
    SettingsFileLoadReply(SettingsFileLoadReply),
    #[serde(rename = "pi.{pi_id}.settings.printnanny.apply")]
    SettingsFileApplyReply(SettingsFileApplyReply),
    #[serde(rename = "pi.{pi_id}.settings.printnanny.revert")]
    SettingsFileRevertReply(SettingsFileRevertReply),

    #[serde(rename = "pi.{pi_id}.settings.camera.apply")]
    CameraSettingsFileApplyReply(VideoStreamSettings),
    #[serde(rename = "pi.{pi_id}.settings.camera.load")]
    CameraSettingsFileLoadReply(VideoStreamSettings),

    // pi.{pi_id}.dbus.org.freedesktop.systemd1.*
    #[serde(rename = "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.DisableUnit")]
    SystemdManagerDisableUnitsReply(SystemdManagerDisableUnitsReply),
    #[serde(rename = "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.EnableUnit")]
    SystemdManagerEnableUnitsReply(SystemdManagerEnableUnitsReply),
    #[serde(rename = "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.GetUnit")]
    SystemdManagerGetUnitReply(SystemdManagerGetUnitReply),
    #[serde(rename = "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.GetUnitFileState")]
    SystemdManagerGetUnitFileStateReply(SystemdManagerGetUnitFileStateReply),
    // TODO: : Job type reload is not applicable for unit octoprint.service.
    // #[serde(rename = "pi.dbus.org.freedesktop.systemd1.Manager.ReloadUnit")]
    // SystemdManagerReloadUnitReply(SystemdManagerReloadUnitReply),
    #[serde(rename = "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.RestartUnit")]
    SystemdManagerRestartUnitReply(SystemdManagerRestartUnitReply),
    #[serde(rename = "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.StartUnit")]
    SystemdManagerStartUnitReply(SystemdManagerStartUnitReply),
    #[serde(rename = "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.StopUnit")]
    SystemdManagerStopUnitReply(SystemdManagerStopUnitReply),
}

impl NatsRequest {
    pub fn handle_camera_recording_load() -> Result<NatsReply> {
        let recordings: Vec<printnanny_asyncapi_models::VideoRecording> =
            printnanny_edge_db::video_recording::VideoRecording::get_all()?
                .into_iter()
                .map(|v| (v).into())
                .collect();
        let current = printnanny_edge_db::video_recording::VideoRecording::get_current()?
            .map(|v| Box::new(v.into()));
        Ok(NatsReply::CameraRecordingLoadReply(
            CameraRecordingLoadReply {
                recordings,
                current,
            },
        ))
    }

    pub async fn handle_camera_recording_start() -> Result<NatsReply> {
        let recording = printnanny_edge_db::video_recording::VideoRecording::start_new()?;
        let factory = PrintNannyPipelineFactory::default();
        factory
            .start_video_recording_pipeline(&recording.mp4_file_name)
            .await?;
        let now = Utc::now();
        let update = printnanny_edge_db::video_recording::UpdateVideoRecording {
            recording_status: Some("inprogress"),
            recording_start: Some(&now),
            deleted: None,
            gcode_file_name: None,
            recording_end: None,
            mp4_upload_url: None,
            mp4_download_url: None,
            cloud_sync_percent: None,
            cloud_sync_status: None,
            cloud_sync_start: None,
            cloud_sync_end: None,
        };
        printnanny_edge_db::video_recording::VideoRecording::update(&recording.id, update)?;
        let recording =
            printnanny_edge_db::video_recording::VideoRecording::get_by_id(&recording.id)?;

        Ok(NatsReply::CameraRecordingStartReply(
            CameraRecordingStarted {
                recording: Box::new(recording.into()),
            },
        ))
    }

    pub async fn handle_camera_recording_stop() -> Result<NatsReply> {
        let recording = printnanny_edge_db::video_recording::VideoRecording::get_current()?;
        let factory = PrintNannyPipelineFactory::default();

        // send EOS signal to gstreamer
        factory.stop_video_recording_pipeline().await?;

        // start cloud sync service
        let settings = PrintNannySettings::new()?;
        if settings.video_stream.recording.cloud_sync {
            match &recording {
                Some(recording) => {
                    let connection = zbus::Connection::system().await?;
                    let proxy = zbus_systemd::systemd1::ManagerProxy::new(&connection).await?;
                    let unit_name = format!(
                        "{PRINTNANNY_RECORDING_SERVICE_TEMPLATE}{}.service",
                        recording.id
                    );
                    info!("Attempting to start {}", &unit_name);
                    // ref: https://www.freedesktop.org/wiki/Software/systemd/dbus/
                    // StartUnit() enqeues a start job, and possibly depending jobs. Takes the unit to activate, plus a mode string.
                    // The mode needs to be one of replace, fail, isolate, ignore-dependencies, ignore-requirements.
                    // If "replace" the call will start the unit and its dependencies, possibly replacing already queued jobs that conflict with this.
                    // If "fail" the call will start the unit and its dependencies, but will fail if this would change an already queued job.
                    // If "isolate" the call will start the unit in question and terminate all units that aren't dependencies of it.
                    // If "ignore-dependencies" it will start a unit but ignore all its dependencies.
                    // If "ignore-requirements" it will start a unit but only ignore the requirement dependencies.
                    // It is not recommended to make use of the latter two options. Returns the newly created job object.
                    let job = proxy.start_unit(unit_name.to_string(), "fail".into()).await; // "fail"
                    match job {
                        Ok(job) => {
                            info!(
                                "Success, submitted StartUnit job={} for unit={}",
                                job.to_string(),
                                &unit_name
                            );
                        }
                        Err(e) => {
                            error!(
                                "Error submitting StartUnit job for {} error={}",
                                &unit_name, e
                            );
                        }
                    }
                }
                None => {
                    warn!("handle_camera_recording_stop called, but no active recording was found. You may need to manually run `printnanny cloud sync-video-recordings` to backup recording to PrintNanny Cloud.");
                }
            }
        }

        let recording = match recording {
            Some(recording) => {
                let now = Utc::now();
                let update = printnanny_edge_db::video_recording::UpdateVideoRecording {
                    recording_status: Some("done"),
                    recording_end: Some(&now),
                    deleted: None,
                    recording_start: None,
                    gcode_file_name: None,
                    mp4_upload_url: None,
                    mp4_download_url: None,
                    cloud_sync_percent: None,
                    cloud_sync_status: None,
                    cloud_sync_start: None,
                    cloud_sync_end: None,
                };
                printnanny_edge_db::video_recording::VideoRecording::update(&recording.id, update)?;
                let recording =
                    printnanny_edge_db::video_recording::VideoRecording::get_by_id(&recording.id)?;
                Some(recording)
            }
            None => None,
        };
        Ok(NatsReply::CameraRecordingStopReply(
            CameraRecordingStopped {
                recording: recording.map(|v| Box::new(v.into())),
            },
        ))
    }

    pub async fn handle_cloud_sync() -> Result<NatsReply> {
        let start = chrono::offset::Utc::now().to_rfc3339();

        let api = ApiService::new()?;
        // sync cloud models to edge db
        api.sync().await?;
        // set optional pipelines to correct state
        let gst_pipelines = PrintNannyPipelineFactory::default();
        gst_pipelines.sync_optional_pipelines().await?;
        let end = chrono::offset::Utc::now().to_rfc3339();

        Ok(NatsReply::PrintNannyCloudSyncReply(
            PrintNannyCloudSyncReply { start, end },
        ))
    }

    // message messages sent to: "pi.{pi_id}.device_info.load"
    pub async fn handle_device_info_load() -> Result<NatsReply> {
        let settings = PrintNannySettings::new()?;
        let issue = fs::read_to_string(settings.paths.issue_txt).await?;
        let os_release = fs::read_to_string(settings.paths.os_release).await?;

        let ifaddrs = tokio::task::spawn_blocking(|| match nix::ifaddrs::getifaddrs() {
            Ok(result) => result
                .map(
                    |v| printnanny_settings::printnanny_asyncapi_models::NetworkInterfaceAddress {
                        interface_name: v.interface_name,
                        flags: v.flags.bits(),
                        address: v.address.map(|v| v.to_string()),
                        netmask: v.netmask.map(|v| v.to_string()),
                        destination: v.destination.map(|v| v.to_string()),
                        broadcast: v.broadcast.map(|v| v.to_string()),
                    },
                )
                .collect(),
            Err(e) => {
                error!("Error loading ifaddrs {}", e.to_string());
                vec![]
            }
        })
        .await?;

        // let ifaddrs = ifaddrs
        //     .map(
        //         |v| printnanny_settings::printnanny_asyncapi_models::NetworkInterfaceAddress {
        //             interface_name: v.interface_name,
        //             flags: v.flags.bits(),
        //             address: v.address.map(|v| v.to_string()),
        //             netmask: v.netmask.map(|v| v.to_string()),
        //             destination: v.destination.map(|v| v.to_string()),
        //             broadcast: v.broadcast.map(|v| v.to_string()),
        //         },
        //     )
        //     .collect();

        Ok(NatsReply::DeviceInfoLoadReply(DeviceInfoLoadReply {
            issue,
            os_release,
            printnanny_cli_version: "".into(), // TODO
            ifaddrs,
        }))
    }

    // handle messages sent to: "pi.{pi_id}.settings.printnanny.cloud.auth"
    pub async fn handle_printnanny_cloud_auth(
        request: &PrintNannyCloudAuthRequest,
    ) -> Result<NatsReply> {
        let api_service = ApiService::new()?;
        let result = api_service
            .connect_cloud_account(request.api_url.clone(), request.api_token.clone())
            .await;

        let result = match result {
            Ok(_) => {
                info!(
                    "Successfully connected PrintNanny Cloud account: {}",
                    request.email
                );
                NatsReply::PrintNannyCloudAuthReply(PrintNannyCloudAuthReply {
                    status_code: 200,
                    msg: format!("Success! Connected account: {}", request.email),
                })
            }
            Err(e) => {
                error!("Failed to connect PrintNanny Cloud account, error: {}", e);
                NatsReply::PrintNannyCloudAuthReply(PrintNannyCloudAuthReply {
                    status_code: 403,
                    msg: format!("Error connecting account: {}", e),
                })
            }
        };
        Ok(result)
    }

    pub async fn handle_crash_report(request: &CrashReportOsLogsRequest) -> Result<NatsReply> {
        let api_service = ApiService::new()?;
        let result = api_service.crash_report_update(&request.id).await?;
        Ok(NatsReply::CrashReportOsLogsReply(CrashReportOsLogsReply {
            id: result.id,
            updated_dt: result.updated_dt,
        }))
    }

    pub fn handle_cameras_load() -> Result<NatsReply> {
        let cameras: Vec<printnanny_asyncapi_models::Camera> =
            CameraVideoSource::from_libcamera_list()?
                .iter()
                .map(|v| v.into())
                .collect();

        Ok(NatsReply::CameraLoadReply(
            printnanny_asyncapi_models::cameras_load_reply::CamerasLoadReply { cameras },
        ))
    }

    pub async fn handle_printnanny_settings_revert(
        request: &SettingsFileRevertRequest,
    ) -> Result<NatsReply> {
        let settings = PrintNannySettings::new()?;

        // revert commit
        let oid = git2::Oid::from_str(&request.git_commit)?;
        settings.git_revert_hooks(Some(oid)).await?;
        let files = vec![settings.to_payload(SettingsApp::Printnanny).await?];
        Self::build_settings_revert_reply(request, &settings, files)
    }

    async fn handle_octoprint_settings_revert(
        request: &SettingsFileRevertRequest,
    ) -> Result<NatsReply> {
        let settings = PrintNannySettings::new()?;
        // revert commit
        let oid = git2::Oid::from_str(&request.git_commit)?;
        settings.octoprint.git_revert_hooks(Some(oid)).await?;
        let files = vec![
            settings
                .octoprint
                .to_payload(SettingsApp::Octoprint)
                .await?,
        ];
        Self::build_settings_revert_reply(request, &settings, files)
    }

    async fn handle_moonraker_settings_revert(
        request: &SettingsFileRevertRequest,
    ) -> Result<NatsReply> {
        let settings = PrintNannySettings::new()?;
        // revert commit
        let oid = git2::Oid::from_str(&request.git_commit)?;
        settings.moonraker.git_revert_hooks(Some(oid)).await?;
        let files = vec![
            settings
                .moonraker
                .to_payload(SettingsApp::Moonraker)
                .await?,
        ];
        Self::build_settings_revert_reply(request, &settings, files)
    }

    async fn handle_klipper_settings_revert(
        request: &SettingsFileRevertRequest,
    ) -> Result<NatsReply> {
        let settings = PrintNannySettings::new()?;
        // revert commit
        let oid = git2::Oid::from_str(&request.git_commit)?;
        settings.klipper.git_revert_hooks(Some(oid)).await?;
        let files = vec![settings.klipper.to_payload(SettingsApp::Klipper).await?];
        Self::build_settings_revert_reply(request, &settings, files)
    }

    fn build_settings_revert_reply(
        request: &SettingsFileRevertRequest,
        settings: &PrintNannySettings,
        files: Vec<SettingsFile>,
    ) -> Result<NatsReply> {
        let git_head_commit = settings.get_git_head_commit()?.oid;
        let git_history: Vec<printnanny_asyncapi_models::GitCommit> =
            settings.get_rev_list()?.iter().map(|r| r.into()).collect();
        Ok(NatsReply::SettingsFileRevertReply(
            SettingsFileRevertReply {
                app: request.app.clone(),
                files,
                git_head_commit,
                git_history,
            },
        ))
    }

    async fn handle_printnanny_settings_apply(
        request: &SettingsFileApplyRequest,
    ) -> Result<NatsReply> {
        let settings = PrintNannySettings::new()?;

        settings
            .save_and_commit(&request.file.content, Some(request.git_commit_msg.clone()))
            .await?;
        let file = settings.to_payload(SettingsApp::Printnanny).await?;
        Self::build_settings_apply_reply(request, settings, file)
    }

    async fn handle_octoprint_settings_apply(
        request: &SettingsFileApplyRequest,
    ) -> Result<NatsReply> {
        let settings = PrintNannySettings::new()?;
        settings
            .octoprint
            .save_and_commit(&request.file.content, Some(request.git_commit_msg.clone()))
            .await?;
        let file = settings
            .octoprint
            .to_payload(SettingsApp::Octoprint)
            .await?;
        Self::build_settings_apply_reply(request, settings, file)
    }

    async fn handle_moonraker_settings_apply(
        request: &SettingsFileApplyRequest,
    ) -> Result<NatsReply> {
        let settings = PrintNannySettings::new()?;
        settings
            .moonraker
            .save_and_commit(&request.file.content, Some(request.git_commit_msg.clone()))
            .await?;
        let file = settings
            .moonraker
            .to_payload(SettingsApp::Moonraker)
            .await?;
        Self::build_settings_apply_reply(request, settings, file)
    }

    async fn handle_klipper_settings_apply(
        request: &SettingsFileApplyRequest,
    ) -> Result<NatsReply> {
        let settings = PrintNannySettings::new()?;
        settings
            .klipper
            .save_and_commit(&request.file.content, Some(request.git_commit_msg.clone()))
            .await?;
        let file = settings.klipper.to_payload(SettingsApp::Klipper).await?;
        Self::build_settings_apply_reply(request, settings, file)
    }

    fn build_settings_apply_reply(
        _request: &SettingsFileApplyRequest,
        settings: PrintNannySettings,
        file: SettingsFile,
    ) -> Result<NatsReply> {
        let git_head_commit = settings.get_git_head_commit()?.oid;
        let git_history: Vec<printnanny_asyncapi_models::GitCommit> =
            settings.get_rev_list()?.iter().map(|r| r.into()).collect();
        Ok(NatsReply::SettingsFileApplyReply(SettingsFileApplyReply {
            file: Box::new(file),
            git_head_commit,
            git_history,
        }))
    }

    async fn handle_printnanny_settings_load() -> Result<Vec<SettingsFile>> {
        let settings = PrintNannySettings::new()?;
        let files = vec![settings.to_payload(SettingsApp::Printnanny).await?];
        Ok(files)
    }

    async fn handle_octoprint_settings_load() -> Result<Vec<SettingsFile>> {
        let settings = PrintNannySettings::new()?;
        let files = vec![
            settings
                .octoprint
                .to_payload(SettingsApp::Octoprint)
                .await?,
        ];
        Ok(files)
    }

    async fn handle_moonraker_settings_load() -> Result<Vec<SettingsFile>> {
        let settings = PrintNannySettings::new()?;
        let files = vec![
            settings
                .moonraker
                .to_payload(SettingsApp::Moonraker)
                .await?,
        ];
        Ok(files)
    }

    async fn handle_klipper_settings_load() -> Result<Vec<SettingsFile>> {
        let settings = PrintNannySettings::new()?;
        let files = vec![settings.klipper.to_payload(SettingsApp::Klipper).await?];
        Ok(files)
    }

    pub async fn handle_settings_load() -> Result<NatsReply> {
        let settings = PrintNannySettings::new()?;

        let git_head_commit = settings.get_git_head_commit()?.oid;
        let git_history: Vec<printnanny_asyncapi_models::GitCommit> =
            settings.get_rev_list()?.iter().map(|r| r.into()).collect();

        let mut files = Self::handle_printnanny_settings_load().await?;
        files.extend(Self::handle_octoprint_settings_load().await?);
        files.extend(Self::handle_moonraker_settings_load().await?);
        files.extend(Self::handle_klipper_settings_load().await?);
        Ok(NatsReply::SettingsFileLoadReply(SettingsFileLoadReply {
            files,
            git_head_commit,
            git_history,
        }))
    }

    pub async fn handle_settings_apply(request: &SettingsFileApplyRequest) -> Result<NatsReply> {
        match *request.file.app {
            SettingsApp::Printnanny => Self::handle_printnanny_settings_apply(request).await,
            SettingsApp::Octoprint => Self::handle_octoprint_settings_apply(request).await,
            SettingsApp::Moonraker => Self::handle_moonraker_settings_apply(request).await,
            SettingsApp::Klipper => Self::handle_klipper_settings_apply(request).await,
        }
    }

    pub async fn handle_camera_settings_load() -> Result<NatsReply> {
        let settings = PrintNannySettings::new()?;
        Ok(NatsReply::CameraSettingsFileLoadReply(
            settings.video_stream.into(),
        ))
    }

    pub async fn handle_camera_settings_apply(request: &VideoStreamSettings) -> Result<NatsReply> {
        info!("Received request: {:#?}", request);
        let mut settings = PrintNannySettings::new()?;

        settings.video_stream = request.clone().into();
        let content = settings.to_toml_string()?;
        let ts = SystemTime::now();
        let commit_msg = format!("Updated PrintNannySettings.camera @ {ts:?}");
        settings.save_and_commit(&content, Some(commit_msg)).await?;
        Ok(NatsReply::CameraSettingsFileApplyReply(
            settings.video_stream.into(),
        ))
    }

    pub async fn handle_settings_revert(request: &SettingsFileRevertRequest) -> Result<NatsReply> {
        match *request.app {
            SettingsApp::Printnanny => Self::handle_printnanny_settings_revert(request).await,
            SettingsApp::Octoprint => Self::handle_octoprint_settings_revert(request).await,
            SettingsApp::Moonraker => Self::handle_moonraker_settings_revert(request).await,
            SettingsApp::Klipper => Self::handle_klipper_settings_revert(request).await,
        }
    }

    pub async fn handle_disable_units_request(
        request: &SystemdManagerUnitFilesRequest,
    ) -> Result<NatsReply> {
        let connection = zbus::Connection::system().await?;
        let proxy = zbus_systemd::systemd1::ManagerProxy::new(&connection).await?;
        let changes = proxy
            .disable_unit_files(request.files.clone(), false)
            .await?;
        let changes = changes
            .iter()
            .map(
                |(change_type, file, destination)| match change_type.as_str() {
                    "symlink" => SystemdUnitChange {
                        change: Box::new(SystemdUnitChangeState::Symlink),
                        file: file.to_string(),
                        destination: destination.to_string(),
                    },
                    "unlink" => SystemdUnitChange {
                        change: Box::new(SystemdUnitChangeState::Symlink),
                        file: file.to_string(),
                        destination: destination.to_string(),
                    },
                    _ => {
                        unimplemented!("No implementation for systemd change type {}", change_type)
                    }
                },
            )
            .collect();
        info!(
            "Disabled units: {:?} - changes: {:?}",
            request.files, changes
        );
        proxy.reload().await?;

        Ok(NatsReply::SystemdManagerDisableUnitsReply(
            SystemdManagerDisableUnitsReply {
                changes,
                request: Box::new(request.clone()),
            },
        ))
    }

    pub async fn handle_enable_units_request(
        request: &SystemdManagerUnitFilesRequest,
    ) -> Result<NatsReply> {
        let connection = zbus::Connection::system().await?;

        let proxy = zbus_systemd::systemd1::ManagerProxy::new(&connection).await?;
        let (_enablement_info, changes) = proxy
            .enable_unit_files(request.files.clone(), false, false)
            .await?;

        let changes = changes
            .iter()
            .map(
                |(change_type, file, destination)| match change_type.as_str() {
                    "symlink" => SystemdUnitChange {
                        change: Box::new(SystemdUnitChangeState::Symlink),
                        file: file.to_string(),
                        destination: destination.to_string(),
                    },
                    "unlink" => SystemdUnitChange {
                        change: Box::new(SystemdUnitChangeState::Symlink),
                        file: file.to_string(),
                        destination: destination.to_string(),
                    },
                    _ => {
                        unimplemented!("No implementation for systemd change type {}", change_type)
                    }
                },
            )
            .collect();
        info!(
            "Enabled units: {:?} - changes: {:?}",
            request.files, changes
        );
        proxy.reload().await?;

        Ok(NatsReply::SystemdManagerEnableUnitsReply(
            SystemdManagerEnableUnitsReply {
                changes,
                request: Box::new(request.clone()),
            },
        ))
    }

    async fn get_systemd_unit(
        unit_name: String,
    ) -> Result<printnanny_asyncapi_models::SystemdUnit> {
        let connection = zbus::Connection::system().await?;
        let proxy = printnanny_dbus::zbus_systemd::systemd1::ManagerProxy::new(&connection).await?;
        let unit_path = proxy.load_unit(unit_name.clone()).await?; // load_unit is similar to get_unit, but will first attempt to load unit file
        let unit =
            printnanny_dbus::systemd1::models::SystemdUnit::from_owned_object_path(unit_path)
                .await?;
        let unit = printnanny_asyncapi_models::SystemdUnit::from(unit);
        Ok(unit)
    }

    async fn handle_get_unit_request(request: &SystemdManagerGetUnitRequest) -> Result<NatsReply> {
        let unit = Self::get_systemd_unit(request.unit_name.clone()).await?;
        Ok(NatsReply::SystemdManagerGetUnitReply(
            SystemdManagerGetUnitReply {
                unit: Box::new(unit),
            },
        ))
    }

    async fn handle_get_unit_file_state_request(
        request: &SystemdManagerGetUnitRequest,
    ) -> Result<NatsReply> {
        let connection = zbus::Connection::system().await?;
        let proxy = printnanny_dbus::zbus_systemd::systemd1::ManagerProxy::new(&connection).await?;

        let unit_file_state = proxy.get_unit_file_state(request.unit_name.clone()).await?;

        let unit_file_state = match unit_file_state.as_str() {
            "enabled" => SystemdUnitFileState::Enabled,
            "enabled-runtime" => SystemdUnitFileState::EnabledMinusRuntime,
            "linked" => SystemdUnitFileState::Linked,
            "linked-runtime" => SystemdUnitFileState::LinkedMinusRuntime,
            "masked" => SystemdUnitFileState::Masked,
            "masked-runtime" => SystemdUnitFileState::MaskedMinusRuntime,
            "static" => SystemdUnitFileState::Static,
            "disabled" => SystemdUnitFileState::Disabled,
            "invalid" => SystemdUnitFileState::Invalid,
            _ => unimplemented!(),
        };

        Ok(NatsReply::SystemdManagerGetUnitFileStateReply(
            SystemdManagerGetUnitFileStateReply {
                unit_file_state: Box::new(unit_file_state),
                request: Box::new(request.clone()),
            },
        ))
    }

    // TODO
    // Job type reload is not applicable for unit octoprint.service.
    // async fn handle_reload_unit_request(
    //     &self,
    //     request: &SystemdManagerReloadUnitRequest,
    // ) -> Result<NatsReply> {
    //     let connection = zbus::Connection::system().await?;
    //     let proxy = zbus_systemd::systemd1::ManagerProxy::new(&connection).await?;
    //     let job = proxy
    //         .reload_unit(request.unit_name.clone(), "replace".into())
    //         .await?;
    //     let unit = self.get_systemd_unit(request.unit_name.clone()).await?;

    //     Ok(NatsReply::SystemdManagerReloadUnitReply(
    //         SystemdManagerReloadUnitReply {
    //             job: job.to_string(),
    //             unit: Box::new(unit),
    //         },
    //     ))
    // }

    async fn handle_restart_unit_request(
        request: &SystemdManagerRestartUnitRequest,
    ) -> Result<NatsReply> {
        let connection = zbus::Connection::system().await?;
        let proxy = zbus_systemd::systemd1::ManagerProxy::new(&connection).await?;
        let job = proxy
            .restart_unit(request.unit_name.clone(), "replace".into())
            .await?;
        let unit = Self::get_systemd_unit(request.unit_name.clone()).await?;

        Ok(NatsReply::SystemdManagerRestartUnitReply(
            SystemdManagerRestartUnitReply {
                job: job.to_string(),
                unit: Box::new(unit),
            },
        ))
    }

    async fn handle_start_unit_request(
        request: &SystemdManagerStartUnitRequest,
    ) -> Result<NatsReply> {
        let connection = zbus::Connection::system().await?;
        let proxy = zbus_systemd::systemd1::ManagerProxy::new(&connection).await?;
        let job = proxy
            .start_unit(request.unit_name.clone(), "replace".into())
            .await?;
        let unit = Self::get_systemd_unit(request.unit_name.clone()).await?;
        Ok(NatsReply::SystemdManagerStartUnitReply(
            SystemdManagerStartUnitReply {
                job: job.to_string(),
                unit: Box::new(unit),
            },
        ))
    }

    async fn handle_stop_unit_request(
        request: &SystemdManagerStopUnitRequest,
    ) -> Result<NatsReply> {
        let connection = zbus::Connection::system().await?;
        let proxy = zbus_systemd::systemd1::ManagerProxy::new(&connection).await?;
        let job = proxy
            .stop_unit(request.unit_name.clone(), "replace".into())
            .await?;
        let unit = Self::get_systemd_unit(request.unit_name.clone()).await?;
        Ok(NatsReply::SystemdManagerStopUnitReply(
            SystemdManagerStopUnitReply {
                job: job.to_string(),
                unit: Box::new(unit),
            },
        ))
    }
}

#[async_trait]
impl NatsRequestHandler for NatsRequest {
    type Request = NatsRequest;
    type Reply = NatsReply;

    fn deserialize_payload(subject_pattern: &str, payload: &Bytes) -> Result<Self::Request> {
        match subject_pattern {
            "pi.{pi_id}.command.camera.recording.start" => {
                Ok(NatsRequest::CameraRecordingStartRequest)
            }
            "pi.{pi_id}.command.camera.recording.stop" => {
                Ok(NatsRequest::CameraRecordingStopRequest)
            }
            "pi.{pi_id}.command.camera.recording.load" => {
                Ok(NatsRequest::CameraRecordingLoadRequest)
            }
            "pi.{pi_id}.command.cloud.sync" => Ok(NatsRequest::PrintNannyCloudSyncRequest),
            "pi.{pi_id}.crash_reports.os" => Ok(NatsRequest::CrashReportOsLogsRequest(
                serde_json::from_slice::<CrashReportOsLogsRequest>(payload.as_ref())?,
            )),
            "pi.{pi_id}.cameras.load" => Ok(NatsRequest::CameraLoadRequest),
            "pi.{pi_id}.device_info.load" => Ok(NatsRequest::DeviceInfoLoadRequest),
            "pi.{pi_id}.settings.printnanny.cloud.auth" => {
                Ok(NatsRequest::PrintNannyCloudAuthRequest(
                    serde_json::from_slice::<PrintNannyCloudAuthRequest>(payload.as_ref())?,
                ))
            }
            "pi.{pi_id}.settings.file.load" => Ok(NatsRequest::SettingsFileLoadRequest),
            "pi.{pi_id}.settings.file.apply" => Ok(NatsRequest::SettingsFileApplyRequest(
                serde_json::from_slice::<SettingsFileApplyRequest>(payload.as_ref())?,
            )),
            "pi.{pi_id}.settings.file.revert" => Ok(NatsRequest::SettingsFileRevertRequest(
                serde_json::from_slice::<SettingsFileRevertRequest>(payload.as_ref())?,
            )),
            "pi.{pi_id}.settings.camera.apply" => Ok(NatsRequest::CameraSettingsFileApplyRequest(
                serde_json::from_slice::<VideoStreamSettings>(payload.as_ref())?,
            )),
            "pi.{pi_id}.settings.camera.load" => Ok(NatsRequest::CameraSettingsFileLoadRequest),
            "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.DisableUnit" => {
                Ok(NatsRequest::SystemdManagerDisableUnitsRequest(
                    serde_json::from_slice::<SystemdManagerUnitFilesRequest>(payload.as_ref())?,
                ))
            }
            "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.EnableUnit" => {
                Ok(NatsRequest::SystemdManagerEnableUnitsRequest(
                    serde_json::from_slice::<SystemdManagerUnitFilesRequest>(payload.as_ref())?,
                ))
            }
            "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.GetUnit" => {
                Ok(NatsRequest::SystemdManagerGetUnitRequest(
                    serde_json::from_slice::<SystemdManagerGetUnitRequest>(payload.as_ref())?,
                ))
            }
            "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.GetUnitFileState" => {
                Ok(NatsRequest::SystemdManagerGetUnitFileStateRequest(
                    serde_json::from_slice::<SystemdManagerGetUnitRequest>(payload.as_ref())?,
                ))
            }
            "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.RestartUnit" => {
                Ok(NatsRequest::SystemdManagerRestartUnitRequest(
                    serde_json::from_slice::<SystemdManagerRestartUnitRequest>(payload.as_ref())?,
                ))
            }
            "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.StartUnit" => {
                Ok(NatsRequest::SystemdManagerStartUnitRequest(
                    serde_json::from_slice::<SystemdManagerStartUnitRequest>(payload.as_ref())?,
                ))
            }
            "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.StopUnit" => {
                Ok(NatsRequest::SystemdManagerStopUnitRequest(
                    serde_json::from_slice::<SystemdManagerStopUnitRequest>(payload.as_ref())?,
                ))
            }
            _ => Err(anyhow!(
                "NATS message handler not implemented for subject pattern {}",
                subject_pattern
            )),
        }
    }

    // Request handlers with blocking I/O should be run with tokio::task::spawn_blocking
    async fn handle(&self) -> Result<Self::Reply> {
        match self {
            // pi.{pi_id}.command.camera.recording.start
            NatsRequest::CameraRecordingStartRequest => Self::handle_camera_recording_start().await,
            // pi.{pi_id}.command.camera.recording.stop
            NatsRequest::CameraRecordingStopRequest => Self::handle_camera_recording_stop().await,
            // pi.{pi_id}.command.camera.recording.load
            NatsRequest::CameraRecordingLoadRequest => {
                tokio::task::spawn_blocking(Self::handle_camera_recording_load).await?
            }
            // pi.{pi_id}.command.cloud.sync
            NatsRequest::PrintNannyCloudSyncRequest => Self::handle_cloud_sync().await,
            // pi.{pi_id}.cameras.load
            NatsRequest::CameraLoadRequest => {
                tokio::task::spawn_blocking(Self::handle_cameras_load).await?
            }
            // "pi.{pi_id}.crash_reports.os"
            NatsRequest::CrashReportOsLogsRequest(request) => {
                Self::handle_crash_report(request).await
            }
            // pi.{pi_id}.device_info.load
            NatsRequest::DeviceInfoLoadRequest => Self::handle_device_info_load().await,

            // pi.{pi_id}.settings.*
            NatsRequest::PrintNannyCloudAuthRequest(request) => {
                Self::handle_printnanny_cloud_auth(request).await
            }
            NatsRequest::SettingsFileLoadRequest => Self::handle_settings_load().await,
            NatsRequest::SettingsFileApplyRequest(request) => {
                Self::handle_settings_apply(request).await
            }
            NatsRequest::SettingsFileRevertRequest(request) => {
                Self::handle_settings_revert(request).await
            }

            NatsRequest::CameraSettingsFileLoadRequest => Self::handle_camera_settings_load().await,

            NatsRequest::CameraSettingsFileApplyRequest(request) => {
                Self::handle_camera_settings_apply(request).await
            }
            // pi.{pi_id}.dbus.org.freedesktop.systemd1.*
            NatsRequest::SystemdManagerDisableUnitsRequest(request) => {
                Self::handle_disable_units_request(request).await
            }
            NatsRequest::SystemdManagerEnableUnitsRequest(request) => {
                Self::handle_enable_units_request(request).await
            }
            NatsRequest::SystemdManagerGetUnitRequest(request) => {
                Self::handle_get_unit_request(request).await
            }
            NatsRequest::SystemdManagerGetUnitFileStateRequest(request) => {
                Self::handle_get_unit_file_state_request(request).await
            }
            NatsRequest::SystemdManagerRestartUnitRequest(request) => {
                Self::handle_restart_unit_request(request).await
            }
            NatsRequest::SystemdManagerStartUnitRequest(request) => {
                Self::handle_start_unit_request(request).await
            }
            NatsRequest::SystemdManagerStopUnitRequest(request) => {
                Self::handle_stop_unit_request(request).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_log::test;
    use tokio::runtime::Runtime;

    #[cfg(test)]
    fn make_settings_repo(jail: &mut figment::Jail) -> () {
        let output = jail.directory().to_str().unwrap();

        jail.create_file(
            "PrintNannySettingsTest.toml",
            &format!(
                r#"
            [paths]
            settings_dir = "{output}/settings"
            state_dir = "{output}/"
            log_dir = "{output}/log"
            "#,
                output = &output
            ),
        )
        .unwrap();
        jail.set_env("PRINTNANNY_SETTINGS", "PrintNannySettingsTest.toml");
        let settings = PrintNannySettings::new().unwrap();
        settings.get_git_repo().unwrap();
    }

    #[test]
    fn test_replace_subject_pattern() {
        let subject = NatsRequest::replace_subject_pattern(
            "pi.localhost.dbus.org.freedesktop.systemd1.Manager.GetUnit",
            "localhost",
            "{pi_id}",
        );
        assert_eq!(
            subject,
            "pi.{pi_id}.dbus.org.freedesktop.systemd1.Manager.GetUnit"
        )
    }

    #[test(tokio::test)]
    async fn test_device_info_load() {
        let request = NatsRequest::DeviceInfoLoadRequest;

        let reply = request.handle().await.unwrap();
        if let NatsReply::DeviceInfoLoadReply(_reply) = reply {
        } else {
            panic!("Expected NatsReply::DeviceInfoLoadReply")
        }
    }

    #[cfg(feature = "systemd")]
    #[test_log::test]
    fn test_printnanny_cloud_auth_failed() {
        figment::Jail::expect_with(|jail| {
            // init git repo in jail tmp dir
            make_settings_repo(jail);
            let email = "testing@test.com".to_string();
            let api_url = "http://localhost:8080/".to_string();
            let api_token = "test_token".to_string();
            let request = NatsRequest::PrintNannyCloudAuthRequest(PrintNannyCloudAuthRequest {
                email,
                api_url,
                api_token,
            });
            let reply = Runtime::new().unwrap().block_on(request.handle()).unwrap();
            if let NatsReply::PrintNannyCloudAuthReply(reply) = reply {
                assert_eq!(reply.status_code, 403);
            } else {
                panic!("Expected NatsReply::PrintNannyCloudAuthReply")
            }
            Ok(())
        })
    }

    #[test_log::test]
    fn test_camera_settings_load() {
        figment::Jail::expect_with(|jail| {
            // init git repo in jail tmp dir
            make_settings_repo(jail);
            // get settings
            let settings = PrintNannySettings::new().unwrap();
            let request = NatsRequest::CameraSettingsFileLoadRequest;

            let reply = Runtime::new().unwrap().block_on(request.handle()).unwrap();
            if let NatsReply::CameraSettingsFileLoadReply(reply) = reply {
                let expected: printnanny_asyncapi_models::VideoStreamSettings =
                    settings.video_stream.into();
                assert_eq!(expected, reply)
            }
            Ok(())
        })
    }

    #[cfg(feature = "systemd")]
    #[test_log::test]
    fn test_camera_settings_apply_load_revert() {
        figment::Jail::expect_with(|jail| {
            // init git repo in jail tmp dir
            make_settings_repo(jail);

            // apply a settings change
            let settings = PrintNannySettings::new().unwrap();
            let mut modified = settings.video_stream.clone();
            modified.hls.enabled = false;

            let request = NatsRequest::CameraSettingsFileApplyRequest(modified.clone().into());
            let reply = Runtime::new().unwrap().block_on(request.handle()).unwrap();

            if let NatsReply::CameraSettingsFileApplyReply(reply) = reply {
                assert_eq!(reply.hls.enabled, false);
                let settings = PrintNannySettings::new().unwrap();
                assert_eq!(settings.video_stream.hls.enabled, false);
            } else {
                panic!("Expected NatsReply::CameraSettingsFileApplyReply")
            }
            Ok(())
        })
    }

    #[cfg(feature = "systemd")]
    #[test_log::test]
    fn test_printnanny_settings_apply_load_revert() {
        figment::Jail::expect_with(|jail| {
            // init git repo in jail tmp dir
            make_settings_repo(jail);

            // apply a settings change
            let mut settings = PrintNannySettings::new().unwrap();
            let original = settings.to_payload(SettingsApp::Printnanny).unwrap();
            let mut modified = original.clone();
            let git_head_commit = settings.get_git_head_commit().unwrap().oid;
            settings.paths.log_dir = "/path/to/testing".into();
            modified.content = settings.to_toml_string().unwrap();
            let git_commit_msg = "testing".to_string();

            let request_apply = NatsRequest::SettingsFileApplyRequest(SettingsFileApplyRequest {
                file: Box::new(modified.clone()),
                git_head_commit,
                git_commit_msg: git_commit_msg.clone(),
            });
            let reply = Runtime::new()
                .unwrap()
                .block_on(request_apply.handle())
                .unwrap();
            let revert_commit = settings.get_git_head_commit().unwrap().oid;

            if let NatsReply::SettingsFileApplyReply(reply) = reply {
                assert_eq!(reply.git_history[0].message, git_commit_msg);
                assert_eq!(reply.git_head_commit, revert_commit);
                assert_eq!(reply.file.content, modified.content);
            } else {
                panic!("Expected NatsReply::SettingsFileApplyReply")
            }

            // load the settings we just applied
            let request_load = NatsRequest::SettingsFileLoadRequest;
            let reply = Runtime::new()
                .unwrap()
                .block_on(request_load.handle())
                .unwrap();
            let reply = if let NatsReply::SettingsFileLoadReply(reply) = reply {
                assert_eq!(reply.git_history[0].message, git_commit_msg);
                assert_eq!(reply.git_head_commit, revert_commit);
                reply
            } else {
                panic!("Expected NatsReply::SettingsFileLoadReply")
            };

            // revert the settings
            let request_revert =
                NatsRequest::SettingsFileRevertRequest(SettingsFileRevertRequest {
                    git_commit: revert_commit,
                    app: Box::new(SettingsApp::Printnanny),
                    files: reply.files,
                });
            let reply = Runtime::new()
                .unwrap()
                .block_on(request_revert.handle())
                .unwrap();
            if let NatsReply::SettingsFileRevertReply(reply) = reply {
                let settings = PrintNannySettings::new().unwrap();

                assert_eq!(reply.files[0].content, settings.to_toml_string().unwrap());
            } else {
                panic!("Expected NatsReply::SettingsFileRevertReply")
            }

            Ok(())
        })
    }

    #[cfg(feature = "systemd")]
    #[test_log::test]
    fn test_octoprint_settings_apply_load_revert() {
        const OCTOPRINT_MODIFIED_SETTINGS: &str = r#"
        ---
        server:
          commands:
            systemShutdownCommand: sudo shutdown -h now
            systemRestartCommand: sudo shutdown -r now
            serverRestartCommand: sudo systemctl restart octoprint.service
        
        api:
          disabled: true
        
        system:
          actions:
            - name: Start PrintNanny Cam
              action: printnanny_cam_start
              command: sudo systemctl restart printnanny-vision.service
            - name: Stop PrintNanny Cam
              action: printnanny_cam_stop
              command: sudo systemctl stop printnanny-vision.service
        events:
          subscriptions:
            - command: sudo systemctl start printnanny-vision.service
              debug: false
              event: plugin_octoprint_nanny_vision_start
              type: system
              enabled: true
            - command: sudo systemctl stop printnanny-vision.service
              enabled: true
              debug: false
              event: plugin_octoprint_nanny_vision_stop
              type: system
        
        webcam:
          stream: /printnanny-hls/playlist.m3u8
        "#;
        figment::Jail::expect_with(|jail| {
            // init git repo in jail tmp dir
            make_settings_repo(jail);

            let settings = PrintNannySettings::new().unwrap();

            // apply a settings change
            let original = settings
                .octoprint
                .to_payload(SettingsApp::Octoprint)
                .unwrap();
            let mut modified = original.clone();
            modified.content = OCTOPRINT_MODIFIED_SETTINGS.into();
            let git_head_commit = settings.get_git_head_commit().unwrap().oid;
            let git_commit_msg = "testing".to_string();

            let request_apply = NatsRequest::SettingsFileApplyRequest(SettingsFileApplyRequest {
                file: Box::new(modified.clone()),
                git_head_commit,
                git_commit_msg: git_commit_msg.clone(),
            });
            let reply = Runtime::new()
                .unwrap()
                .block_on(request_apply.handle())
                .unwrap();
            let revert_commit = settings.get_git_head_commit().unwrap().oid;
            if let NatsReply::SettingsFileApplyReply(reply) = reply {
                assert_eq!(reply.git_history[0].message, git_commit_msg);
                assert_eq!(reply.git_head_commit, revert_commit);
                assert_eq!(reply.file.content, modified.content);
            } else {
                panic!("Expected NatsReply::SettingsFileApplyReply")
            }

            // load the settings we just applied
            let request_load = NatsRequest::SettingsFileLoadRequest;
            let reply = Runtime::new()
                .unwrap()
                .block_on(request_load.handle())
                .unwrap();
            let reply = if let NatsReply::SettingsFileLoadReply(reply) = reply {
                assert_eq!(reply.git_history[0].message, git_commit_msg);
                assert_eq!(reply.git_head_commit, revert_commit);
                reply
            } else {
                panic!("Expected NatsReply::SettingsFileLoadReply")
            };

            // revert the settings
            let request_revert =
                NatsRequest::SettingsFileRevertRequest(SettingsFileRevertRequest {
                    git_commit: revert_commit,
                    app: Box::new(SettingsApp::Octoprint),
                    files: reply.files,
                });
            let reply = Runtime::new()
                .unwrap()
                .block_on(request_revert.handle())
                .unwrap();
            if let NatsReply::SettingsFileRevertReply(reply) = reply {
                assert_eq!(reply.files[0].content, original.content);
            } else {
                panic!("Expected NatsReply::SettingsFileRevertReply")
            }

            Ok(())
        });
    }

    #[cfg(feature = "systemd")]
    #[test_log::test]
    fn test_moonraker_settings_apply_load_revert() {
        const MOONRAKER_MODIFIED_SETTINGS: &str = r#"
        # https://github.com/Arksine/moonraker/blob/master/docs/installation.md
        [server]
        host: 0.0.0.0
        port: 7125
        klippy_uds_address: /var/run/klipper/klippy.sock
        
        [machine]
        validate_service: false
        provider: systemd_dbus
        
        [authorization]
        cors_domains:
            https://my.mainsail.xyz
            http://my.mainsail.xyz
            http://*.local
            http://*.lan
        
        trusted_clients:
            10.0.0.0/8
            127.0.0.0/8
            169.254.0.0/16
            172.16.0.0/12
            192.168.0.0/16
            FE80::/10
            ::1/128
        
        # enables partial support of Octoprint API
        [octoprint_compat]
        
        # enables moonraker to track and store print history.
        [history]
        "#;
        figment::Jail::expect_with(|jail| {
            // init git repo in jail tmp dir
            make_settings_repo(jail);

            let settings = PrintNannySettings::new().unwrap();

            // apply a settings change
            let original = settings
                .moonraker
                .to_payload(SettingsApp::Octoprint)
                .unwrap();
            let mut modified = original.clone();
            modified.content = MOONRAKER_MODIFIED_SETTINGS.into();
            let git_head_commit = settings.get_git_head_commit().unwrap().oid;
            let git_commit_msg = "testing".to_string();

            let request_apply = NatsRequest::SettingsFileApplyRequest(SettingsFileApplyRequest {
                file: Box::new(modified.clone()),
                git_head_commit,
                git_commit_msg: git_commit_msg.clone(),
            });
            let reply = Runtime::new()
                .unwrap()
                .block_on(request_apply.handle())
                .unwrap();
            let revert_commit = settings.get_git_head_commit().unwrap().oid;
            if let NatsReply::SettingsFileApplyReply(reply) = reply {
                assert_eq!(reply.git_history[0].message, git_commit_msg);
                assert_eq!(reply.git_head_commit, revert_commit);
                assert_eq!(reply.file.content, modified.content);
            } else {
                panic!("Expected NatsReply::SettingsFileApplyReply")
            }

            // load the settings we just applied
            let request_load = NatsRequest::SettingsFileLoadRequest;
            let reply = Runtime::new()
                .unwrap()
                .block_on(request_load.handle())
                .unwrap();
            let reply = if let NatsReply::SettingsFileLoadReply(reply) = reply {
                assert_eq!(reply.git_history[0].message, git_commit_msg);
                assert_eq!(reply.git_head_commit, revert_commit);
                reply
            } else {
                panic!("Expected NatsReply::SettingsFileLoadReply")
            };

            // revert the settings
            let request_revert =
                NatsRequest::SettingsFileRevertRequest(SettingsFileRevertRequest {
                    git_commit: revert_commit,
                    app: Box::new(SettingsApp::Moonraker),
                    files: reply.files,
                });
            let reply = Runtime::new()
                .unwrap()
                .block_on(request_revert.handle())
                .unwrap();
            if let NatsReply::SettingsFileRevertReply(reply) = reply {
                assert_eq!(reply.files[0].content, original.content);
            } else {
                panic!("Expected NatsReply::SettingsFileRevertReply")
            }

            Ok(())
        });
    }

    #[cfg(feature = "systemd")]
    #[test_log::test(tokio::test)] // async test
    async fn test_dbus_systemd_manager_get_unit_file_state_ok() {
        let request =
            NatsRequest::SystemdManagerGetUnitFileStateRequest(SystemdManagerGetUnitRequest {
                unit_name: "octoprint.service".into(),
            });
        let reply = request.handle().await.unwrap();
        if let NatsReply::SystemdManagerGetUnitFileStateReply(reply) = reply {
            // unit may already be in an enabled stateSystemdManagerUnitFilesRequest
            assert!(
                *reply.unit_file_state == SystemdUnitFileState::Enabled
                    || *reply.unit_file_state == SystemdUnitFileState::Disabled
            );
        } else {
            panic!("Expected NatsReply::SystemdManagerGetUnit")
        }
    }

    #[cfg(feature = "systemd")]
    #[test_log::test(tokio::test)] // async test
    async fn test_dbus_systemd_manager_get_unit_file_state_error() {
        let request =
            NatsRequest::SystemdManagerGetUnitFileStateRequest(SystemdManagerGetUnitRequest {
                unit_name: "doesnotexist.service".into(),
            });
        let reply = request.handle().await;
        assert!(reply.is_err());
    }

    #[cfg(feature = "systemd")]
    #[test_log::test(tokio::test)] // async test
    async fn test_dbus_systemd_manager_enable_disable_unit_ok() {
        let request =
            NatsRequest::SystemdManagerEnableUnitsRequest(SystemdManagerUnitFilesRequest {
                files: vec!["octoprint.service".into()],
            });
        let natsreply = request.handle().await.unwrap();
        if let NatsReply::SystemdManagerEnableUnitsReply(reply) = natsreply {
            // unit may already be in an enabled state
            assert!(reply.changes.len() == 1 || reply.changes.len() == 0);
        } else {
            panic!("Expected NatsReply::SystemdManagerEnableUnitReply")
        }

        let request =
            NatsRequest::SystemdManagerDisableUnitsRequest(SystemdManagerUnitFilesRequest {
                files: vec!["octoprint.service".into()],
            });
        let natsreply = request.handle().await.unwrap();
        if let NatsReply::SystemdManagerDisableUnitsReply(reply) = natsreply {
            // unit is guaranteed to be in enabled state from prior request
            assert_eq!(reply.changes.len(), 1);
        } else {
            panic!("Expected NatsReply::SystemdManagerDisableUnitReply")
        }
    }

    #[cfg(feature = "systemd")]
    #[test_log::test(tokio::test)] // async test
    async fn test_dbus_systemd_manager_disable_unit_error() {
        let request = SystemdManagerUnitFilesRequest {
            files: vec!["doesnotexist.service".into()],
        };
        let natsrequest = NatsRequest::SystemdManagerDisableUnitsRequest(request.clone());
        let natsreply = natsrequest.handle().await;
        assert!(natsreply.is_err());
    }

    #[cfg(feature = "systemd")]
    #[test_log::test(tokio::test)] // async test
    async fn test_dbus_systemd_manager_enable_unit_error() {
        let request = SystemdManagerUnitFilesRequest {
            files: vec!["doesnotexist.service".into()],
        };
        let natsrequest = NatsRequest::SystemdManagerEnableUnitsRequest(request.clone());
        let natsreply = natsrequest.handle().await;
        assert!(natsreply.is_err());
    }

    #[cfg(feature = "systemd")]
    #[test_log::test(tokio::test)] // async test
    async fn test_dbus_systemd_get_unit_error() {
        let request = NatsRequest::SystemdManagerGetUnitRequest(SystemdManagerGetUnitRequest {
            unit_name: "doesnotexist.service".into(),
        });
        let reply = request.handle().await;
        assert!(reply.is_err());
    }

    #[cfg(feature = "systemd")]
    #[test_log::test(tokio::test)] // async test
    async fn test_dbus_systemd_restart_unit_error() {
        let request =
            NatsRequest::SystemdManagerRestartUnitRequest(SystemdManagerRestartUnitRequest {
                unit_name: "doesnotexist.service".into(),
            });
        let reply = request.handle().await;
        assert!(reply.is_err());
    }
    #[cfg(feature = "systemd")]
    #[test_log::test(tokio::test)] // async test
    async fn test_dbus_systemd_reload_unit_ok() {
        let request =
            NatsRequest::SystemdManagerRestartUnitRequest(SystemdManagerRestartUnitRequest {
                unit_name: "octoprint.service".into(),
            });
        let reply = request.handle().await.unwrap();
        if let NatsReply::SystemdManagerRestartUnitReply(reply) = reply {
            assert_eq!(
                *(*reply.unit).load_state,
                printnanny_asyncapi_models::SystemdUnitLoadState::Loaded
            );
        } else {
            panic!("Expected NatsReply::SystemdManagerRestartUniReply")
        }
    }

    #[cfg(feature = "systemd")]
    #[test_log::test(tokio::test)] // async test
    async fn test_dbus_systemd_start_unit_error() {
        let request = NatsRequest::SystemdManagerStartUnitRequest(SystemdManagerStartUnitRequest {
            unit_name: "doesnotexist.service".into(),
        });
        let reply = request.handle().await;
        assert!(reply.is_err());
    }

    #[cfg(feature = "systemd")]
    #[test_log::test(tokio::test)] // async test
    async fn test_dbus_systemd_start_unit_ok() {
        let request = NatsRequest::SystemdManagerStartUnitRequest(SystemdManagerStartUnitRequest {
            unit_name: "octoprint.service".into(),
        });
        let reply = request.handle().await.unwrap();
        if let NatsReply::SystemdManagerStartUnitReply(reply) = reply {
            assert_eq!(
                *(*reply.unit).load_state,
                printnanny_asyncapi_models::SystemdUnitLoadState::Loaded
            );
        } else {
            panic!("Expected NatsReply::SystemdManagerStartUnitReply")
        }
    }

    #[cfg(feature = "systemd")]
    #[test_log::test(tokio::test)] // async test
    async fn test_dbus_systemd_stop_unit_error() {
        let request = NatsRequest::SystemdManagerStopUnitRequest(SystemdManagerStopUnitRequest {
            unit_name: "doesnotexist.service".into(),
        });
        let reply = request.handle().await;
        assert!(reply.is_err());
    }

    #[cfg(feature = "systemd")]
    #[test_log::test(tokio::test)] // async test
    async fn test_dbus_systemd_stop_unit_ok() {
        let request =
            NatsRequest::SystemdManagerEnableUnitsRequest(SystemdManagerUnitFilesRequest {
                files: vec!["octoprint.service".into()],
            });
        let natsreply = request.handle().await.unwrap();
        if let NatsReply::SystemdManagerEnableUnitsReply(reply) = natsreply {
            // unit may already be in an enabled state
            assert!(reply.changes.len() == 1 || reply.changes.len() == 0);
        } else {
            panic!("Expected NatsReply::SystemdManagerEnableUnitReply")
        }
        request.handle().await.unwrap();

        let request = NatsRequest::SystemdManagerStopUnitRequest(SystemdManagerStopUnitRequest {
            unit_name: "octoprint.service".into(),
        });
        let reply = request.handle().await.unwrap();
        if let NatsReply::SystemdManagerStopUnitReply(reply) = reply {
            assert_eq!(
                *(*reply.unit).load_state,
                printnanny_asyncapi_models::SystemdUnitLoadState::Loaded
            );
        } else {
            panic!("Expected NatsReply::SystemdManagerStopUnitReply")
        }
    }
}
