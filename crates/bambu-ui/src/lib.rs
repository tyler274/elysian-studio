#![forbid(unsafe_code)]

mod chrome;
#[cfg(target_os = "linux")]
mod desktop;
mod filament;
mod inventory;
mod monitor;
mod plater;
mod sidebar;
mod snapshot;
pub mod theme;

pub use snapshot::{
    decode_png, encode_png, header_has_fill, png_delta, scale_rgba, sidebar_is_width, write_png,
    GuiSnapshot,
};

use bambu_alloc as _;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use bambu_config::{
    apply_sku_with_generic_base, clone_filament_as_user, delete_user_filament, list_bbl_profiles,
    list_filament_json_dir, list_instantiated_bbl_profiles, list_studio_user_filaments,
    load_bbl_process, load_default_catalog, overlay_bbl_profile, patch_filament_colour,
    patch_user_filament_settings, profile_filament_id, resolve_ams_filament, BblProfileEntry,
    BblProfileKind, CatalogFilament, CatalogIndex, FilamentMapMode, SeamPosition, SliceSettings,
    TopOneWallType,
};
use bambu_device::{AmsState, AmsTray, AmsUnit, MachineState, PrintJob, PrinterBackend};
use bambu_gcode::{parse_gcode, write_gcode, write_gcode_for_objects};
use bambu_gpu::{
    force_vulkan_env, paint_overlay_color, probe_vulkan, screen_axis_len,
    slice_volumes_with_gpu_or_cpu, slice_with_gpu_or_cpu, AxisGizmo, CameraView, ExtrusionRole,
    Meshlet, PlaterTool, ShadedSolids, ToolpathBuffer, ViewportEvent, ViewportScene,
};
use bambu_io::{
    load_mesh, load_model_with_progress, read_3mf_thumbnail, write_model_3mf,
    write_model_3mf_bytes_with_thumbnail, LoadStage,
};
use bambu_model::{Model, TrianglePaint};
use bambu_protocol::{
    camera_serial_from_bind, describe_hms, load_cached_catalog, load_cloud_session, load_inventory,
    load_lan_codes, refresh_catalog, save_cloud_session, save_inventory, save_lan_codes, CloudApi,
    CloudBackend, CloudDevice, FilamentSpool, Inventory, LanBackend, LoginResult, ProjectFileOpts,
    StudioPrinter,
};
use bambu_slicer::{check_print_path_conflicts, compute_filament_map, GroupSlot, GroupTray};
use iced::widget::{button, checkbox, column, container, row, stack, text};
use iced::{window, Element, Fill, Settings, Size, Subscription, Task, Theme};

/// Studio `MainFrame::SetSize(FromDIP(1200), FromDIP(800))` — 3:2, not iced's 4:3.
pub const WINDOW_SIZE: Size = Size::new(1200.0, 800.0);
const WINDOW_MIN: Size = Size::new(960.0, 640.0);
pub const SIDEBAR_WIDTH: f32 = 300.0;

use plater::{CoordSpace, XformField};

pub fn run() -> iced::Result {
    // KWin reads app_id from the session XDG dirs, not from winit's pixel icon.
    // Publish before the Vulkan re-exec so kded can index the files while the
    // child probes the GPU.
    #[cfg(target_os = "linux")]
    desktop::publish();
    // ICD + loader lookup must be set before the WGPU_BACKEND re-exec so the
    // child inherits NVIDIA's JSON instead of Mesa nouveau/lvp.
    force_vulkan_env();
    reexec_with_vulkan_if_needed();

    tracing_subscriber::fmt()
        .with_env_filter({
            let mut filter = tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("bambu_ui=info".parse().unwrap())
                .add_directive("bambu_gpu=info".parse().unwrap());
            #[cfg(debug_assertions)]
            {
                filter = filter
                    .add_directive("bambu_protocol=debug".parse().unwrap())
                    .add_directive("bambu_ui=debug".parse().unwrap());
            }
            filter
        })
        .init();

    force_vulkan_env();
    let report = probe_vulkan().unwrap_or_else(|err| {
        panic!("Vulkan adapter required on this host: {err}");
    });

    #[cfg(target_os = "linux")]
    if !report.is_vulkan {
        panic!(
            "expected Vulkan backend, got {} ({})",
            report.backend, report.name
        );
    }

    tracing::info!("GPU adapter: {} backend={}", report.name, report.backend);

    let adapter = format!("{} / {}", report.backend, report.name);
    let mut window = window::Settings {
        size: WINDOW_SIZE,
        min_size: Some(WINDOW_MIN),
        icon: window_icon(),
        ..window::Settings::default()
    };
    #[cfg(target_os = "linux")]
    {
        window.platform_specific.application_id = String::from(desktop::APP_ID);
    }
    iced::application(
        move || {
            let app = App::new(adapter.clone());
            let boot = if app.has_bearer {
                Task::done(Message::RefreshDevices)
            } else {
                Task::none()
            };
            (app, boot)
        },
        App::update,
        App::view,
    )
    .subscription(App::subscription)
    .title("Bambu Studio")
    .theme(App::theme)
    .style(|_, t| theme::window(t))
    .settings(Settings {
        antialiasing: true,
        default_text_size: iced::Pixels(13.0),
        id: Some(String::from("bambu-studio-rs")),
        ..Settings::default()
    })
    .window(window)
    .centered()
    .run()
}

fn window_icon() -> Option<window::Icon> {
    let (width, height, rgba) = decode_png(include_bytes!("../assets/icon.png")).ok()?;
    window::icon::from_rgba(rgba, width, height).ok()
}

fn reexec_with_vulkan_if_needed() {
    if std::env::var("WGPU_BACKEND").ok().as_deref() == Some("vulkan") {
        return;
    }

    let current_exe = std::env::current_exe().expect("current_exe");
    let mut cmd = Command::new(&current_exe);
    cmd.env("WGPU_BACKEND", "vulkan")
        .args(std::env::args_os().skip(1));

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        panic!("failed to re-exec with WGPU_BACKEND=vulkan: {err}");
    }

    #[cfg(not(unix))]
    {
        let status = cmd.status().expect("re-exec with WGPU_BACKEND=vulkan");
        std::process::exit(status.code().unwrap_or(1));
    }
}

pub struct App {
    adapter: String,
    scene: ViewportScene,
    workspace: Workspace,
    busy: bool,
    status: String,
    host: String,
    access_code: String,
    serial: String,
    last_gcode: Option<String>,
    estimated_seconds: Option<f64>,
    settings: SliceSettings,
    model: Option<Arc<Model>>,
    plate: usize,
    by_object: bool,
    paint_kind: Option<PaintKind>,
    paint_blocker: bool,
    brush_mm: f32,
    show_axes: bool,
    gizmo_hover: bool,
    meshlet_gen: u64,
    load_gen: u64,
    load_progress: Option<LoadProgress>,
    object_aabbs: Vec<bambu_geom::Aabb3>,
    needs_display_pack: bool,
    pack_inflight: bool,
    pack_gen: u64,
    mqtt_status: String,
    process_profiles: Vec<BblProfileEntry>,
    filament_profiles: Vec<BblProfileEntry>,
    user_filaments: Vec<BblProfileEntry>,
    studio_filaments: Vec<BblProfileEntry>,
    machine_profiles: Vec<BblProfileEntry>,
    process_name: Option<String>,
    filament_name: Option<String>,
    machine_name: Option<String>,
    filament_slots: Vec<FilamentSlot>,
    active_filament: usize,
    user_preset_name: String,
    filament_page: FilamentPage,
    inventory: Inventory,
    catalog: CatalogIndex,
    catalog_query: String,
    show_bbl_presets: bool,
    show_archived: bool,
    spool_search: String,
    selected_spool: Option<usize>,
    draft_spool: FilamentSpool,
    draft_location: String,
    draft_archived: bool,
    draft_use: String,
    draft_measure: String,
    selected_object: usize,
    selected_volume: usize,
    drag_last_bed: Option<(f32, f32)>,
    drag_last_ndc: Option<(f32, f32)>,
    drag_axis: Option<bambu_gpu::GizmoAxis>,
    machine: MachineState,
    ams: AmsState,
    hms_lines: Vec<String>,
    live_monitor: bool,
    monitor_fetching: bool,
    camera_live: bool,
    stop_confirm: bool,
    slice_all_next: Option<usize>,
    send_via: SendVia,
    cloud_user: String,
    cloud_region: String,
    has_bearer: bool,
    imported_printers: Vec<StudioPrinter>,
    cloud_devices: Vec<CloudDevice>,
    selected_device: Option<String>,
    login_account: String,
    login_password: String,
    login_code: String,
    chamber_handle: Option<iced::widget::image::Handle>,
    chamber_width: u32,
    chamber_height: u32,
    camera_note: String,
    control_bed: String,
    control_nozzle: String,
    control_fan: u8,
    project_opts: ProjectFileOpts,
    coord_space: CoordSpace,
    uniform_scale: bool,
    rotate_open_deg: glam::Vec3,
    pos_edit: [String; 3],
    rot_rel_edit: [String; 3],
    rot_abs_edit: [String; 3],
    scale_pct_edit: [String; 3],
    size_edit: [String; 3],
    quality_tab: ProcessTab,
    process_search: String,
    process_objects: bool,
    fps: f32,
    toasts: Vec<String>,
    slice_menu_open: bool,
    print_menu_open: bool,
    file_menu_open: bool,
    slice_all: bool,
    print_export: bool,
    recent_models: Vec<PathBuf>,
    recent_thumbs: HashMap<(PathBuf, u64), iced::widget::image::Handle>,
    project_file: Option<PathBuf>,
    dry_ams: Option<u8>,
    dry_temp: String,
    dry_hours: String,
    dry_rotate: bool,
    rack_pending: Option<RackPending>,
    sidebar_collapsed: bool,
    printer_open: bool,
    filament_open: bool,
    process_open: bool,
}

#[allow(private_interfaces)]
#[derive(Debug, Clone)]
pub enum Message {
    Viewport(ViewportEvent),
    Workspace(Workspace),
    OpenModel,
    ImportModel,
    NewProject,
    SaveProject,
    SaveProjectAs,
    ToggleFileMenu,
    FileDropped(PathBuf),
    ProjectPicked(Option<PathBuf>),
    ProjectSaved(Result<PathBuf, String>),
    OpenRecent(PathBuf),
    MeshPicked(Option<PathBuf>),
    LoadProgress {
        gen: u64,
        label: String,
        fraction: f32,
    },
    ModelLoaded(Result<Box<LoadedModel>, String>),
    Slice,
    ToggleSliceMenu,
    SliceAll(bool),
    TogglePrintMenu,
    PrintExport(bool),
    Sliced(Result<Box<SliceOutcome>, String>),
    ResetCamera,
    CameraView(CameraView),
    CollapseSidebar,
    ExtractKeys,
    KeysExtracted(Result<ExtractUi, String>),
    Discover,
    Discovered(Result<Vec<bambu_protocol::DiscoveredPrinter>, String>),
    Host(String),
    AccessCode(String),
    Serial(String),
    Send,
    Sent(Result<(), String>),
    ExportPicked(Option<PathBuf>),
    Chamber,
    CameraPlay,
    CameraStop,
    CameraLan(Result<Vec<bambu_protocol::DiscoveredPrinter>, String>),
    ChamberShot(Result<ChamberResult, String>),
    WallLoops(u32),
    Infill(f64),
    LayerHeight(f64),
    NozzleTemp(f64),
    Plate(usize),
    PreviewLayer(u32),
    HideInfill(bool),
    HideSupport(bool),
    Realistic(bool),
    ByObject(bool),
    PaintSupport,
    PaintSeam,
    PaintFuzzy,
    PaintClear,
    PaintBlocker(bool),
    BrushRadius(f32),
    ShowAxes(bool),
    MeshletsReady {
        gen: u64,
        meshlets: Vec<Vec<Meshlet>>,
    },
    DisplayPacked {
        gen: u64,
        shaded: Box<ShadedSolids>,
    },
    PreviewMove(u32),
    EnableSupport(bool),
    RefreshStatus,
    Status(Result<Box<MonitorSnapshot>, String>),
    Pause,
    Resume,
    Stop,
    StopConfirm,
    StopCancel,
    PrintControl(Result<String, String>),
    LiveMonitor(bool),
    SendVia(SendVia),
    ImportStudio,
    StudioImported(Result<Box<StudioImportUi>, String>),
    RefreshDevices,
    DevicesLoaded(Result<Vec<CloudDevice>, String>),
    PickDevice(String),
    LoginAccount(String),
    LoginPassword(String),
    LoginCode(String),
    CloudLogin,
    CloudOAuth,
    CloudLogged(Result<String, String>),
    PrintSpeed(u8),
    ChamberLight(bool),
    BedSet(String),
    NozzleSet(String),
    FanSet(f64),
    SendBed,
    SendNozzle,
    SendFan,
    AmsLoad {
        ams_id: u8,
        slot_id: u8,
    },
    AmsUnload {
        ams_id: u8,
    },
    AmsDryToggle(u8),
    AmsDryTemp(String),
    AmsDryHours(String),
    AmsDryRotate(bool),
    AmsDryStart(u8),
    AmsDryStop(u8),
    RackMove(u8),
    RackReadAll,
    RackConfirmAll,
    RackWarnConfirm,
    RackWarnCancel,
    HmsResume,
    HmsIgnore,
    ProjectBedLevel(bool),
    ProjectFlowCali(bool),
    ProjectVibrationCali(bool),
    ProjectLayerInspect(bool),
    ProjectTimelapse(bool),
    ProcessProfile(String),
    AddFilamentSlot,
    RemoveFilamentSlot,
    SelectFilamentSlot(usize),
    FilamentSlotPreset {
        slot: usize,
        label: String,
    },
    FilamentSlotColour {
        slot: usize,
        colour: String,
    },
    CycleFilamentColour(usize),
    UserPresetName(String),
    SaveUserPreset,
    DeleteUserPreset,
    MachineProfile(String),
    SelectObject(usize),
    SelectVolume(usize),
    HideVolume(bool),
    CycleAmsMap(usize),
    RefreshHms,
    HmsCatalog(Result<String, String>),
    Calibration,
    SyncAms,
    FilamentPage(FilamentPage),
    FilamentMapMode(FilamentMapMode),
    AssignSlotExtruder {
        slot: usize,
        extruder: i32,
    },
    ParamSoluble(bool),
    ParamSupport(bool),
    ParamPaEnable(bool),
    ParamPa(f64),
    ParamRangeLow(f64),
    ParamCost(f64),
    ParamNotes(String),
    ParamAdaptiveVol(bool),
    ParamPrime(f64),
    ParamFlushTemp(f64),
    ParamFlushTempFast(f64),
    ParamFlushVol(f64),
    ParamRammingVol(f64),
    ParamRammingTravel(f64),
    ParamPrecool(f64),
    ParamPrintable(bool),
    ParamStartGcode(String),
    ParamEndGcode(String),
    ParamRetract(f64),
    ParamWipe(bool),
    ParamLongEc(bool),
    CoolingFanMin(f64),
    CoolingFanMax(f64),
    CoolingSlowdown(f64),
    CatalogQuery(String),
    CatalogAdd(String),
    ShowBblPresets(bool),
    ShowArchived(bool),
    SpoolLocation(String),
    SpoolArchived(bool),
    SpoolUseGrams(String),
    SpoolMeasureGross(String),
    ApplySpoolUse,
    ApplySpoolMeasure,
    InventorySearch(String),
    InventoryNew,
    InventorySelect(usize),
    InventorySave,
    InventoryDelete,
    InventoryPull,
    InventoryPush,
    InventoryPulled(Result<Vec<FilamentSpool>, String>),
    InventoryPushed(Result<String, String>),
    BindSpoolToSlot,
    SpoolBrand(String),
    SpoolMaterial(String),
    SpoolSeries(String),
    SpoolColor(String),
    SpoolFilamentId(String),
    SpoolNote(String),
    PlaterTool(PlaterTool),
    Arrange,
    AutoOrient,
    Mirror(u8),
    Rotate90,
    CoordSpace(CoordSpace),
    UniformScale(bool),
    XformDraft {
        field: XformField,
        text: String,
    },
    XformCommit(XformField),
    DropToBed,
    ResetRotation,
    ResetScale,
    ProcessTab(ProcessTab),
    ProcessSearch(String),
    ProcessObjects(bool),
    Seam(SeamPosition),
    SeamAway(bool),
    ScarfConditional(bool),
    ScarfEntireLoop(bool),
    ScarfInnerWalls(bool),
    OverrideScarf(bool),
    FirstLayerHeight(f64),
    PreciseOuterWall(bool),
    OnlyOneWallFirst(bool),
    OnlyOneWallTop(bool),
    PlatePrev,
    PlateNext,
    Toast(String),
    TogglePrinterSection,
    ToggleFilamentSection,
    ToggleProcessSection,
}

impl From<ViewportEvent> for Message {
    fn from(event: ViewportEvent) -> Self {
        Message::Viewport(event)
    }
}

#[derive(Debug, Clone)]
struct MonitorSnapshot {
    line: String,
    machine: MachineState,
    ams: AmsState,
    hms_lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SendVia {
    #[default]
    LanFtps,
    CloudUpload,
}

impl SendVia {
    fn label(self) -> &'static str {
        match self {
            SendVia::LanFtps => "LAN FTPS",
            SendVia::CloudUpload => "Cloud upload",
        }
    }
}

impl std::fmt::Display for SendVia {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Workspace {
    Home,
    #[default]
    Prepare,
    Preview,
    Device,
    Project,
    Calibration,
    Filament,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProcessTab {
    #[default]
    Quality,
    Strength,
    Speed,
    Support,
    Others,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilamentSource {
    System,
    User,
    Studio,
    Catalog,
    Inventory,
}

#[derive(Debug, Clone)]
pub(crate) struct FilamentSlot {
    name: String,
    colour: String,
    source: FilamentSource,
    filament_id: String,
    filament_type: String,
    is_support: bool,
    ams_id: Option<u8>,
    slot_id: Option<u8>,
    external_id: String,
    spool_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum FilamentPage {
    #[default]
    Filament,
    Cooling,
    Overrides,
    Advanced,
    Notes,
    Multi,
}

impl FilamentPage {
    fn label(self) -> &'static str {
        match self {
            Self::Filament => "Filament",
            Self::Cooling => "Cooling",
            Self::Overrides => "Overrides",
            Self::Advanced => "Advanced",
            Self::Notes => "Notes",
            Self::Multi => "Multi",
        }
    }
}

impl std::fmt::Display for FilamentPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone)]
struct ExtractUi {
    can_sign: bool,
    note: String,
}

#[derive(Debug, Clone)]
struct LoadedModel {
    label: String,
    path: PathBuf,
    model: Model,
    apply_settings: bool,
    load_ms: u128,
    shaded: ShadedSolids,
    object_aabbs: Vec<bambu_geom::Aabb3>,
}

#[derive(Debug, Clone)]
struct LoadProgress {
    label: String,
    fraction: f32,
}

#[derive(Clone)]
struct LoadJob {
    gen: u64,
    path: PathBuf,
    apply_settings: bool,
    bed: bambu_config::BedShape,
}

#[derive(Debug, Clone)]
enum SliceOutcome {
    Single {
        result: bambu_slicer::SliceResult,
        backend: String,
        settings: SliceSettings,
    },
    Objects {
        results: Vec<bambu_slicer::SliceResult>,
        settings: SliceSettings,
    },
}

struct SliceJob {
    settings: SliceSettings,
    mesh: bambu_geom::TriangleMesh,
    volumes: Option<Vec<bambu_model::ModelVolume>>,
    objects: Option<Vec<Vec<bambu_model::ModelVolume>>>,
}

#[derive(Debug, Clone)]
enum ChamberResult {
    Jpeg {
        bytes: usize,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Rtsps {
        detail: String,
    },
}

impl ChamberResult {
    fn from_frame(bytes: usize, frame: bambu_device::Frame) -> Self {
        Self::Jpeg {
            bytes,
            width: frame.width,
            height: frame.height,
            rgba: frame.rgba,
        }
    }
}

#[derive(Debug, Clone)]
struct StudioImportUi {
    user_id: String,
    region: String,
    has_token: bool,
    printers: Vec<StudioPrinter>,
    default_serial: String,
    status: String,
    discovered: Vec<bambu_protocol::DiscoveredPrinter>,
}

impl App {
    pub fn new(adapter: String) -> Self {
        let settings = default_slice_settings();
        let bed = settings.bed_shape();
        let mut scene = ViewportScene::with_cube_on_bed(adapter.clone(), bed.orbit_mm());
        scene.set_bed_shape(bed.clone());
        let mut model = Model::from_mesh("cube", bambu_geom::TriangleMesh::cube(20.0));
        model.place_on_bed_if_needed(&bed);
        if let Some(mesh) = model.mesh_for_plate(0) {
            scene.set_mesh(mesh);
        }
        scene.pack_gpu();
        let mut app = Self {
            adapter,
            scene,
            workspace: Workspace::Prepare,
            busy: false,
            status: format!("20mm cube on {:.0}×{:.0} mm bed", bed.width(), bed.height()),
            host: String::new(),
            access_code: String::new(),
            serial: String::new(),
            last_gcode: None,
            estimated_seconds: None,
            settings,
            model: Some(Arc::new(model)),
            plate: 0,
            by_object: false,
            paint_kind: None,
            paint_blocker: false,
            brush_mm: 2.0,
            show_axes: true,
            gizmo_hover: false,
            meshlet_gen: 0,
            load_gen: 0,
            load_progress: None,
            object_aabbs: Vec::new(),
            needs_display_pack: false,
            pack_inflight: false,
            pack_gen: 0,
            mqtt_status: String::new(),
            process_profiles: list_bbl_profiles(BblProfileKind::Process),
            filament_profiles: list_instantiated_bbl_profiles(BblProfileKind::Filament),
            user_filaments: Vec::new(),
            studio_filaments: list_studio_user_filaments(),
            machine_profiles: list_bbl_profiles(BblProfileKind::Machine),
            process_name: None,
            filament_name: None,
            machine_name: None,
            filament_slots: Vec::new(),
            active_filament: 0,
            user_preset_name: String::new(),
            filament_page: FilamentPage::Filament,
            inventory: Inventory::default(),
            catalog: CatalogIndex::default(),
            catalog_query: String::new(),
            show_bbl_presets: false,
            show_archived: false,
            spool_search: String::new(),
            selected_spool: None,
            draft_spool: FilamentSpool::default(),
            draft_location: String::new(),
            draft_archived: false,
            draft_use: String::new(),
            draft_measure: String::new(),
            selected_object: 0,
            selected_volume: 0,
            drag_last_bed: None,
            drag_last_ndc: None,
            drag_axis: None,
            machine: MachineState::default(),
            ams: AmsState::default(),
            hms_lines: Vec::new(),
            live_monitor: false,
            monitor_fetching: false,
            camera_live: false,
            stop_confirm: false,
            slice_all_next: None,
            send_via: SendVia::LanFtps,
            cloud_user: String::new(),
            cloud_region: String::new(),
            has_bearer: false,
            imported_printers: Vec::new(),
            cloud_devices: Vec::new(),
            selected_device: None,
            login_account: String::new(),
            login_password: String::new(),
            login_code: String::new(),
            chamber_handle: None,
            chamber_width: 0,
            chamber_height: 0,
            camera_note: String::new(),
            control_bed: String::new(),
            control_nozzle: String::new(),
            control_fan: 0,
            project_opts: ProjectFileOpts::default(),
            coord_space: CoordSpace::World,
            uniform_scale: true,
            rotate_open_deg: glam::Vec3::ZERO,
            pos_edit: std::array::from_fn(|_| "0.00".into()),
            rot_rel_edit: std::array::from_fn(|_| "0.00".into()),
            rot_abs_edit: std::array::from_fn(|_| "0.00".into()),
            scale_pct_edit: std::array::from_fn(|_| "100.00".into()),
            size_edit: std::array::from_fn(|_| "0.00".into()),
            quality_tab: ProcessTab::Quality,
            process_search: String::new(),
            process_objects: false,
            fps: 60.0,
            toasts: vec!["Tip: right-drag to orbit, scroll to zoom.".into()],
            slice_menu_open: false,
            print_menu_open: false,
            file_menu_open: false,
            slice_all: false,
            print_export: false,
            recent_models: Vec::new(),
            recent_thumbs: HashMap::new(),
            project_file: None,
            dry_ams: None,
            dry_temp: "55".into(),
            dry_hours: "8".into(),
            dry_rotate: false,
            rack_pending: None,
            sidebar_collapsed: false,
            printer_open: true,
            filament_open: true,
            process_open: true,
        };
        app.reload_user_filaments();
        app.catalog = load_default_catalog();
        app.inventory = load_inventory(bambu_protocol::default_config_dir()).unwrap_or_default();
        if let Some(first) = app.inventory.spools.first() {
            app.draft_spool = app.inventory.flatten(first);
            app.draft_location = first.location.clone();
            app.draft_archived = first.archived;
            app.selected_spool = Some(0);
        }
        app.init_filament_slots();
        app.sync_keep_solid();
        app.scene.viewport_height = WINDOW_SIZE.height;
        app.load_ui_prefs();
        app.sync_gizmo();
        app.fill_xform_edits();
        app.object_aabbs = app
            .model
            .as_deref()
            .and_then(|m| m.objects.first())
            .and_then(|o| o.mesh.aabb())
            .into_iter()
            .collect();
        app.load_account_from_disk();
        app.load_recents();
        app
    }

    pub fn new_for_gui_test() -> Self {
        let mut app = Self::new("test-adapter".into());
        app.host.clear();
        app.access_code.clear();
        app.serial.clear();
        app.cloud_user.clear();
        app.inventory = Inventory::default();
        app.filament_slots = vec![Self::empty_slot()];
        app.active_filament = 0;
        app.machine_name = None;
        app.process_name = None;
        app.process_search.clear();
        app.process_objects = false;
        app.quality_tab = ProcessTab::Quality;
        app.status = "20mm cube on bed".into();
        app.has_bearer = false;
        app.live_monitor = false;
        app.camera_live = false;
        app.selected_device = None;
        app.cloud_devices.clear();
        app.stop_confirm = false;
        app.slice_all_next = None;
        app.recent_models.clear();
        app.project_file = None;
        app.file_menu_open = false;
        app.dry_ams = None;
        app.rack_pending = None;
        app.show_axes = true;
        app.gizmo_hover = false;
        app.sync_gizmo();
        app.sync_filament_map();
        app
    }

    pub fn paint_overlay_count(&self) -> usize {
        self.scene.paint_overlay.len()
    }

    pub fn gizmo_visible(&self) -> bool {
        self.scene.gizmo.is_some()
    }

    pub fn show_axes(&self) -> bool {
        self.show_axes
    }

    /// Cube with support-paint blockers for overlay goldens / unit tests.
    pub fn seed_support_paint_cube(&mut self) {
        let mut model = Model::from_mesh("paint", bambu_geom::TriangleMesh::cube(20.0));
        model.place_on_bed_if_needed(&self.scene.bed);
        if let Some(vol) = model.objects.get_mut(0).and_then(|o| o.volumes.get_mut(0)) {
            let n = vol.mesh.indices.len();
            vol.triangle_support = vec![TrianglePaint::None; n];
            vol.triangle_support[0] = TrianglePaint::Blocker;
            vol.triangle_support[1] = TrianglePaint::Blocker;
        }
        self.model = Some(Arc::new(model));
        self.selected_object = 0;
        self.paint_kind = None;
        self.sync_scene_mesh();
    }

    pub fn seed_home_recents(&mut self, path: PathBuf) {
        self.recent_models = vec![path];
        self.refresh_recent_thumbs();
    }

    /// Tiny 3MF with `Metadata/plate_1.png` for Home recents goldens.
    pub fn write_recent_preview_3mf(path: &std::path::Path) -> Result<(), String> {
        let model = Model::from_mesh("preview_cube", bambu_geom::TriangleMesh::cube(20.0));
        let bytes = write_model_3mf_bytes_with_thumbnail(&model, Some(TINY_PNG))
            .map_err(|e| e.to_string())?;
        std::fs::write(path, bytes).map_err(|e| e.to_string())
    }

    /// Deterministic Device StatusPanel for headless goldens (no live MQTT).
    pub fn seed_device_monitor(&mut self) {
        self.has_bearer = false;
        self.host.clear();
        self.access_code.clear();
        self.live_monitor = false;
        self.camera_live = false;
        self.stop_confirm = false;
        self.machine = MachineState {
            online: true,
            gcode_state: "RUNNING".into(),
            gcode_file: "cube.gcode".into(),
            mc_percent: 42,
            layer_num: 48,
            total_layer_num: 120,
            mc_remaining_time_min: 73,
            nozzle_temp_c: 219.0,
            nozzle_target_c: 220.0,
            bed_temp_c: 59.0,
            bed_target_c: 60.0,
            chamber_temp_c: 32.0,
            wifi_signal: "-44dBm".into(),
            spd_lvl: 2,
            cooling_fan: 128,
            ..MachineState::default()
        };
        self.ams = AmsState {
            slot_count: 8,
            active_slot: Some(1),
            humidity: Some(2),
            unit_temp: Some(32.0),
            trays: vec![
                seed_tray(0, 0, "PLA", "00AE42FF", Some(80)),
                seed_tray(0, 1, "PLA", "FF0000FF", Some(55)),
                seed_tray(0, 2, "PETG", "2979FFFF", Some(30)),
                seed_tray(0, 3, "", "", None),
                seed_tray(1, 0, "PLA", "FFFFFFFF", Some(90)),
                seed_tray(1, 1, "ABS", "000000FF", Some(40)),
                seed_tray(1, 2, "TPU", "FF9800FF", Some(20)),
                seed_tray(1, 3, "PLA", "9C27B0FF", Some(10)),
            ],
            units: vec![
                AmsUnit {
                    id: 0,
                    humidity: Some(2),
                    humidity_percent: Some(28),
                    temp: Some(32.0),
                    dry_time_min: Some(90),
                    dry_status: 2,
                    ams_type: 3,
                    ..Default::default()
                },
                AmsUnit {
                    id: 1,
                    humidity: Some(4),
                    humidity_percent: Some(55),
                    temp: Some(27.0),
                    dry_time_min: None,
                    dry_status: 0,
                    ams_type: 3,
                    ..Default::default()
                },
            ],
            ..AmsState::default()
        };
        self.mqtt_status = "RUNNING · 42% · L48/120 · 73m · nozzle 219/220°C · bed 59/60°C · wifi -44dBm · spd 2 RH2".into();
        self.control_bed = "60".into();
        self.control_nozzle = "220".into();
        self.control_fan = 128;
        self.hms_lines.clear();
        self.camera_note.clear();
        self.chamber_handle = None;
        self.chamber_width = 0;
        self.chamber_height = 0;
    }

    /// Keep Preview chrome goldens from auto-reslicing (`drive` drops the slice Task).
    pub fn seed_preview_gcode_placeholder(&mut self) {
        if self.last_gcode.is_none() {
            self.last_gcode = Some("; unsliced".into());
        }
    }

    fn load_account_from_disk(&mut self) {
        let dir = bambu_protocol::default_config_dir();
        if let Ok(session) = load_cloud_session(&dir) {
            self.cloud_user = session.user_id;
            self.cloud_region = session.region;
            self.has_bearer = !session.access_token.is_empty();
            if self.serial.is_empty() {
                self.serial = session.serial;
            }
        }
        let codes = load_lan_codes(&dir);
        if self.access_code.is_empty() {
            if let Some(code) = codes.get(&self.serial) {
                self.access_code = code.clone();
            }
        }
        self.imported_printers = codes
            .into_iter()
            .map(|(serial, access_code)| StudioPrinter {
                serial,
                access_code,
            })
            .collect();
        if !self.serial.is_empty() {
            let serial = self.serial.clone();
            self.apply_device(&serial);
        }
    }

    fn apply_studio_import(&mut self, imported: StudioImportUi) {
        self.imported_printers = imported.printers;
        self.load_account_from_disk();
        if self.serial.is_empty() {
            self.serial = imported.default_serial.clone();
        }
        let serial = self.serial.clone();
        self.apply_device(&serial);
        if self.host.is_empty() {
            if let Some(found) = imported
                .discovered
                .iter()
                .find(|p| p.dev_id == serial)
                .or_else(|| imported.discovered.first())
            {
                self.host = found.dev_ip.clone();
                if self.serial.is_empty() {
                    self.serial = found.dev_id.clone();
                    let serial = self.serial.clone();
                    self.apply_device(&serial);
                }
            }
        }
        self.cloud_user = if imported.user_id.is_empty() {
            self.cloud_user.clone()
        } else {
            imported.user_id
        };
        if !imported.region.is_empty() {
            self.cloud_region = imported.region;
        }
        self.has_bearer = imported.has_token || self.has_bearer;
        self.status = imported.status;
    }

    fn persist_bind_lan_codes(devices: &[CloudDevice]) {
        let dir = bambu_protocol::default_config_dir();
        let mut codes = load_lan_codes(&dir);
        let mut changed = false;
        for device in devices {
            if device.access_code.is_empty() {
                continue;
            }
            if codes.get(&device.dev_id) != Some(&device.access_code) {
                codes.insert(device.dev_id.clone(), device.access_code.clone());
                changed = true;
            }
        }
        if changed {
            let _ = save_lan_codes(&dir, &codes);
        }
    }

    fn apply_device(&mut self, serial: &str) {
        if serial.is_empty() {
            return;
        }
        self.serial = serial.to_string();
        self.selected_device = Some(serial.to_string());
        if let Some(p) = self
            .imported_printers
            .iter()
            .find(|p| p.serial == serial && !p.access_code.is_empty())
        {
            self.access_code = p.access_code.clone();
        }
        if self.access_code.is_empty() {
            if let Some(dev) = self
                .cloud_devices
                .iter()
                .find(|d| d.dev_id == serial && !d.access_code.is_empty())
            {
                self.access_code = dev.access_code.clone();
            }
        }
        if self.access_code.is_empty() {
            if let Some(code) = load_lan_codes(bambu_protocol::default_config_dir()).get(serial) {
                self.access_code = code.clone();
            }
        }
        if let Ok(mut session) = load_cloud_session(bambu_protocol::default_config_dir()) {
            session.serial = serial.to_string();
            let _ = save_cloud_session(bambu_protocol::default_config_dir(), &session);
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![iced::event::listen_with(on_window_event)];
        if self.live_monitor {
            subs.push(
                iced::time::every(std::time::Duration::from_secs(10))
                    .map(|_| Message::RefreshStatus),
            );
        }
        if self.camera_live && self.camera_job_ready() {
            let job = self.camera_job();
            subs.push(Subscription::run_with(job, |job| {
                monitor::camera_frames(job.clone())
            }));
        }
        Subscription::batch(subs)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Viewport(event) => self.handle_viewport(event),
            Message::Workspace(workspace) => {
                let entered_device =
                    workspace == Workspace::Device && self.workspace != Workspace::Device;
                let entered_preview =
                    workspace == Workspace::Preview && self.workspace != Workspace::Preview;
                self.workspace = workspace;
                self.slice_menu_open = false;
                self.print_menu_open = false;
                self.file_menu_open = false;
                self.sync_keep_solid();
                if entered_preview
                    && self.last_gcode.is_none()
                    && self.model.is_some()
                    && !self.busy
                {
                    return self.slice_current();
                }
                if entered_device {
                    return self.start_live_sync();
                }
            }
            Message::OpenModel => {
                if self.busy {
                    self.status = "busy…".into();
                    return Task::none();
                }
                self.file_menu_open = false;
                return Task::perform(pick_mesh_path(), Message::MeshPicked);
            }
            Message::ImportModel => {
                if self.busy {
                    self.status = "busy…".into();
                    return Task::none();
                }
                self.file_menu_open = false;
                return Task::perform(pick_import_path(), Message::MeshPicked);
            }
            Message::NewProject => {
                self.file_menu_open = false;
                return self.new_project();
            }
            Message::SaveProject => {
                self.file_menu_open = false;
                return self.save_project(false);
            }
            Message::SaveProjectAs => {
                self.file_menu_open = false;
                return self.save_project(true);
            }
            Message::ToggleFileMenu => {
                self.file_menu_open = !self.file_menu_open;
                self.slice_menu_open = false;
                self.print_menu_open = false;
            }
            Message::FileDropped(path) => {
                if self.busy {
                    self.status = "busy…".into();
                    return Task::none();
                }
                let apply_settings = is_project_3mf(&path);
                return self.begin_open(path, apply_settings, "loading model…");
            }
            Message::ProjectPicked(None) => {}
            Message::ProjectPicked(Some(path)) => return self.write_project_to(path),
            Message::ProjectSaved(Ok(path)) => {
                self.busy = false;
                self.project_file = Some(path.clone());
                self.remember_recent(path.clone());
                self.status = format!("saved {}", path.display());
            }
            Message::ProjectSaved(Err(err)) => {
                self.busy = false;
                self.status = format!("save failed: {err}");
            }
            Message::OpenRecent(path) => {
                let apply_settings = is_project_3mf(&path);
                return self.begin_open(path, apply_settings, "loading model…");
            }
            Message::MeshPicked(None) => {}
            Message::MeshPicked(Some(path)) => {
                let apply_settings = is_project_3mf(&path);
                return self.begin_open(path, apply_settings, "loading model…");
            }
            Message::LoadProgress {
                gen,
                label,
                fraction,
            } => {
                if gen == self.load_gen {
                    self.status = label.clone();
                    self.load_progress = Some(LoadProgress { label, fraction });
                }
            }
            Message::ModelLoaded(result) => {
                self.busy = false;
                self.load_progress = None;
                match result {
                    Ok(loaded) => return self.apply_loaded_model(*loaded),
                    Err(err) => {
                        self.sync_scene_mesh();
                        self.status = format!("open failed: {err}");
                    }
                }
            }
            Message::ToggleSliceMenu => {
                self.slice_menu_open = !self.slice_menu_open;
                self.print_menu_open = false;
                self.file_menu_open = false;
            }
            Message::SliceAll(all) => {
                self.slice_all = all;
                self.slice_menu_open = false;
            }
            Message::TogglePrintMenu => {
                self.print_menu_open = !self.print_menu_open;
                self.slice_menu_open = false;
                self.file_menu_open = false;
            }
            Message::PrintExport(export) => {
                self.print_export = export;
                self.print_menu_open = false;
            }
            Message::Slice => {
                if self.slice_all {
                    self.slice_all_next = Some(0);
                    self.set_plate(0);
                } else {
                    self.slice_all_next = None;
                }
                return self.slice_current();
            }
            Message::Sliced(result) => {
                self.busy = false;
                match result {
                    Ok(outcome) => {
                        let _ = self.apply_slice_outcome(*outcome);
                        return self.continue_slice_all();
                    }
                    Err(err) => {
                        self.slice_all_next = None;
                        self.slice_all = false;
                        self.status = format!("slice failed: {err}");
                    }
                }
            }
            Message::ResetCamera => {
                let (cx, cy) = self.scene.bed.center();
                self.scene.camera = bambu_gpu::OrbitCamera::looking_at_center(
                    glam::Vec3::new(cx, cy, 0.0),
                    self.scene.bed_mm,
                );
            }
            Message::CameraView(view) => {
                let (cx, cy) = self.scene.bed.center();
                self.scene
                    .camera
                    .apply_view(view, glam::Vec3::new(cx, cy, 0.0), self.scene.bed_mm);
            }
            Message::CollapseSidebar => {
                self.sidebar_collapsed = !self.sidebar_collapsed;
            }
            Message::ExtractKeys => {
                if !self.begin_work("extracting keys…") {
                    return Task::none();
                }
                return offload(
                    || {
                        bambu_protocol::extract_keys(bambu_protocol::ExtractKeysOpts::default())
                            .map(|report| ExtractUi {
                                can_sign: report.credentials.can_sign(),
                                note: report.notes.last().cloned().unwrap_or_default(),
                            })
                            .map_err(|err| err.to_string())
                    },
                    Message::KeysExtracted,
                );
            }
            Message::KeysExtracted(result) => {
                self.busy = false;
                match result {
                    Ok(report) => {
                        let dir = bambu_protocol::default_config_dir();
                        self.status = format!(
                            "keys → {} · sign={} · {}",
                            dir.display(),
                            if report.can_sign {
                                "ready"
                            } else {
                                "missing slicer_key.pem"
                            },
                            report.note
                        );
                    }
                    Err(err) => self.status = format!("extract failed: {err}"),
                }
            }
            Message::ImportStudio => {
                self.status = "Import Studio…".into();
                return Task::perform(
                    async {
                        std::thread::spawn(|| {
                            let imported = bambu_protocol::import_studio(None, None)
                                .map_err(|err| err.to_string())?;
                            let discovered =
                                bambu_protocol::discover(std::time::Duration::from_secs(3))
                                    .unwrap_or_default();
                            let status = imported
                                .status_lines()
                                .into_iter()
                                .take(8)
                                .collect::<Vec<_>>()
                                .join(" · ");
                            Ok(Box::new(StudioImportUi {
                                user_id: imported.user_id,
                                region: imported.region,
                                has_token: imported.has_token,
                                printers: imported.printers,
                                default_serial: imported.default_serial,
                                status,
                                discovered,
                            }))
                        })
                        .join()
                        .unwrap_or_else(|_| Err("import thread panicked".into()))
                    },
                    Message::StudioImported,
                );
            }
            Message::StudioImported(Ok(imported)) => {
                self.apply_studio_import(*imported);
                return self.start_live_sync();
            }
            Message::StudioImported(Err(err)) => {
                self.status = format!("import failed: {err}");
            }
            Message::RefreshDevices => {
                self.status = "cloud bind list…".into();
                return Task::perform(
                    async {
                        std::thread::spawn(|| {
                            let dir = bambu_protocol::default_config_dir();
                            let session =
                                load_cloud_session(&dir).map_err(|err| err.to_string())?;
                            if !session.has_bearer() {
                                return Err("cloud_token missing".into());
                            }
                            let mut api = CloudApi::new(
                                &session.region,
                                &session.access_token,
                                &session.refresh_token,
                            );
                            let devices = api
                                .with_retry(|api| api.list_devices())
                                .map_err(|err| err.to_string())?;
                            if api.access_token != session.access_token {
                                let mut next = session.clone();
                                next.access_token = api.access_token;
                                next.refresh_token = api.refresh_token;
                                let _ = save_cloud_session(&dir, &next);
                            }
                            Ok(devices)
                        })
                        .join()
                        .unwrap_or_else(|_| Err("devices thread panicked".into()))
                    },
                    Message::DevicesLoaded,
                );
            }
            Message::DevicesLoaded(Ok(devices)) => {
                self.cloud_devices = devices;
                Self::persist_bind_lan_codes(&self.cloud_devices);
                let restored = self.restore_bound_printer();
                self.status = format!("{} cloud device(s)", self.cloud_devices.len());
                if restored {
                    return self.start_live_sync();
                }
            }
            Message::DevicesLoaded(Err(err)) => {
                self.status = format!("devices: {err}");
            }
            Message::PickDevice(label) => {
                if let Some(dev) = self
                    .cloud_devices
                    .iter()
                    .find(|d| d.label() == label)
                    .cloned()
                {
                    self.apply_device(&dev.dev_id);
                    return self.start_live_sync();
                }
            }
            Message::LoginAccount(s) => self.login_account = s,
            Message::LoginPassword(s) => self.login_password = s,
            Message::LoginCode(s) => self.login_code = s,
            Message::CloudLogin => {
                let account = self.login_account.clone();
                let password = self.login_password.clone();
                let code = self.login_code.clone();
                let region = if self.cloud_region.is_empty() {
                    "us".into()
                } else {
                    self.cloud_region.clone()
                };
                self.status = "cloud login…".into();
                return Task::perform(
                    async move {
                        std::thread::spawn(move || {
                            let code_opt = if code.is_empty() { None } else { Some(code) };
                            let result =
                                CloudApi::login(&region, &account, &password, code_opt.as_deref())
                                    .map_err(|err| err.to_string())?;
                            match result {
                                LoginResult::NeedsCode { login_type } => {
                                    Err(format!("enter email code ({login_type})"))
                                }
                                LoginResult::Tokens {
                                    access_token,
                                    refresh_token,
                                    user_id,
                                } => {
                                    let dir = bambu_protocol::default_config_dir();
                                    let mut session = load_cloud_session(&dir).unwrap_or_default();
                                    session.region = region;
                                    session.access_token = access_token;
                                    if !refresh_token.is_empty() {
                                        session.refresh_token = refresh_token;
                                    }
                                    if !user_id.is_empty() {
                                        session.user_id = user_id;
                                    }
                                    save_cloud_session(&dir, &session)
                                        .map_err(|err| err.to_string())?;
                                    Ok("logged in (token stored, not shown)".into())
                                }
                            }
                        })
                        .join()
                        .unwrap_or_else(|_| Err("login thread panicked".into()))
                    },
                    Message::CloudLogged,
                );
            }
            Message::CloudOAuth => {
                let region = if self.cloud_region.is_empty() {
                    "us".into()
                } else {
                    self.cloud_region.clone()
                };
                self.status = "browser OAuth… sign in, then return here".into();
                return Task::perform(
                    async move {
                        std::thread::spawn(move || {
                            let result = bambu_protocol::oauth_login(
                                &region,
                                true,
                                std::time::Duration::from_secs(300),
                            )
                            .map_err(|err| err.to_string())?;
                            let dir = bambu_protocol::default_config_dir();
                            bambu_protocol::persist_login(&dir, &region, result)
                                .map_err(|err| err.to_string())?;
                            Ok("logged in via OAuth (token stored, not shown)".into())
                        })
                        .join()
                        .unwrap_or_else(|_| Err("oauth thread panicked".into()))
                    },
                    Message::CloudLogged,
                );
            }
            Message::CloudLogged(Ok(msg)) => {
                self.load_account_from_disk();
                self.login_password.clear();
                self.login_code.clear();
                self.status = msg;
                return Task::done(Message::RefreshDevices);
            }
            Message::CloudLogged(Err(err)) => self.status = format!("login: {err}"),
            Message::Discover => {
                self.status = "SSDP discover on UDP 2021…".into();
                return Task::perform(discover_lan_job(), Message::Discovered);
            }
            Message::Discovered(Ok(list)) if list.is_empty() => {
                self.status = "no printers on UDP 2021 (3s)".into();
            }
            Message::Discovered(Ok(list)) => {
                if let Some(first) = list.first().cloned() {
                    self.host = first.dev_ip.clone();
                    self.apply_device(&first.dev_id);
                }
                self.status = list
                    .iter()
                    .map(|p| format!("{} {}", p.dev_ip, p.dev_name))
                    .collect::<Vec<_>>()
                    .join(" · ");
                return self.start_live_sync();
            }
            Message::Discovered(Err(err)) => {
                self.status = format!("discover failed: {err}");
            }
            Message::Host(s) => {
                self.host = s;
                if self.can_monitor() && !self.live_monitor {
                    return self.start_live_sync();
                }
            }
            Message::AccessCode(s) => {
                self.access_code = s;
                if self.can_monitor() && !self.live_monitor {
                    return self.start_live_sync();
                }
            }
            Message::Serial(s) => self.serial = s,
            Message::Send => {
                let Some(gcode) = self.last_gcode.clone() else {
                    self.status = "slice before send".into();
                    return Task::none();
                };
                if self.print_export {
                    return Task::perform(pick_export_path(), Message::ExportPicked);
                }
                let send_via = self.send_via;
                if send_via == SendVia::LanFtps
                    && (self.host.is_empty() || self.access_code.is_empty())
                {
                    self.status = "printer IP and LAN access code required".into();
                    return Task::none();
                }
                if send_via == SendVia::CloudUpload && !self.has_bearer {
                    self.status =
                        "cloud upload needs a Bearer token (Import Studio or login)".into();
                    return Task::none();
                }
                let host = self.host.clone();
                let code = self.access_code.clone();
                let serial = self.serial.clone();
                let mapping = self.settings.filament_map.clone();
                let opts = self.project_opts;
                self.status = match send_via {
                    SendVia::LanFtps => format!("FTPS + MQTT to {host}…"),
                    SendVia::CloudUpload => "cloud upload + MQTT project_file…".into(),
                };
                return Task::perform(
                    async move {
                        match send_via {
                            SendVia::CloudUpload => {
                                let backend = CloudBackend::from_config_dir(
                                    bambu_protocol::default_config_dir(),
                                )
                                .map_err(|err| err.to_string())?
                                .with_ams_mapping(mapping)
                                .with_project_opts(opts);
                                backend
                                    .start_print(PrintJob {
                                        filename: "plater.gcode".into(),
                                        gcode,
                                    })
                                    .await
                                    .map_err(|err| err.to_string())
                            }
                            SendVia::LanFtps => {
                                let creds = bambu_protocol::load_from_dir(
                                    bambu_protocol::default_config_dir(),
                                )
                                .unwrap_or_else(|_| Default::default());
                                let backend = LanBackend::new(host, code)
                                    .with_serial(serial)
                                    .with_credentials(creds)
                                    .with_ams_mapping(mapping)
                                    .with_project_opts(opts);
                                backend
                                    .start_print(PrintJob {
                                        filename: "plater.gcode".into(),
                                        gcode,
                                    })
                                    .await
                                    .map_err(|err| err.to_string())
                            }
                        }
                    },
                    Message::Sent,
                );
            }
            Message::Sent(Ok(())) => {
                self.status = "print command sent".into();
            }
            Message::Sent(Err(err)) => {
                self.status = format!("send failed: {err}");
            }
            Message::ExportPicked(None) => {}
            Message::ExportPicked(Some(path)) => {
                let Some(gcode) = self.last_gcode.as_deref() else {
                    self.status = "slice before send".into();
                    return Task::none();
                };
                match write_exported_3mf(gcode, &path) {
                    Ok(()) => self.status = format!("exported {}", path.display()),
                    Err(err) => self.status = format!("export failed: {err}"),
                }
            }
            Message::Chamber => {
                if self.host.is_empty() || self.access_code.is_empty() {
                    self.status = "printer IP and LAN access code required".into();
                    return Task::none();
                }
                if self.camera_live {
                    self.status = "live camera already running".into();
                    return Task::none();
                }
                let host = self.host.clone();
                let code = self.access_code.clone();
                self.status = format!("chamber JPEG :6000 / RTSPS :322 {host}…");
                return Task::perform(
                    async move {
                        std::thread::spawn(move || monitor::grab_chamber(host, code))
                            .join()
                            .unwrap_or_else(|_| Err("camera thread panicked".into()))
                    },
                    Message::ChamberShot,
                );
            }
            Message::CameraPlay => return self.play_camera(),
            Message::CameraStop => {
                self.camera_live = false;
                self.status = "camera stopped".into();
            }
            Message::CameraLan(result) => return self.on_camera_lan(result),
            Message::ChamberShot(Ok(ChamberResult::Jpeg {
                bytes,
                width,
                height,
                rgba,
            })) => {
                self.chamber_handle =
                    Some(iced::widget::image::Handle::from_rgba(width, height, rgba));
                self.chamber_width = width;
                self.chamber_height = height;
                self.camera_note.clear();
                if !self.live_monitor {
                    self.status = format!("chamber JPEG {width}×{height} ({bytes} bytes)");
                }
            }
            Message::ChamberShot(Ok(ChamberResult::Rtsps { detail })) => {
                self.chamber_handle = None;
                self.chamber_width = 0;
                self.chamber_height = 0;
                self.camera_note = detail.clone();
                self.status = detail;
            }
            Message::ChamberShot(Err(err)) => {
                self.camera_note = if err.starts_with("camera:") {
                    err.clone()
                } else {
                    format!("camera: {err}")
                };
                if !self.live_monitor {
                    self.status = format!("camera failed: {err}");
                }
            }
            Message::WallLoops(n) => self.settings.wall_loops = n.clamp(1, 10),
            Message::Infill(v) => self.settings.infill_density = v.clamp(0.0, 1.0),
            Message::LayerHeight(v) => self.settings.layer_height_mm = v.clamp(0.08, 0.4),
            Message::NozzleTemp(v) => {
                self.settings.temperature_c = v.round().clamp(160.0, 300.0) as u16;
            }
            Message::Plate(i) => self.set_plate(i),
            Message::PreviewLayer(i) => {
                let max = self.scene.toolpaths.layer_zs.len().saturating_sub(1) as u32;
                self.scene.preview_layer = i.min(max);
                self.scene.preview_vertices = self
                    .scene
                    .toolpaths
                    .vertices_for_layer(self.scene.preview_layer as usize)
                    as u32;
            }
            Message::HideInfill(v) => self.scene.hide_infill = v,
            Message::HideSupport(v) => self.scene.hide_support = v,
            Message::Realistic(v) => self.scene.realistic = v,
            Message::ByObject(v) => self.by_object = v,
            Message::PaintSupport => {
                self.paint_kind = Some(PaintKind::Support);
                self.scene.tool = PlaterTool::Orbit;
                self.scene.keep_solid = true;
                self.status = "click a triangle to paint support".into();
                self.refresh_paint_overlay();
            }
            Message::PaintSeam => {
                self.paint_kind = Some(PaintKind::Seam);
                self.scene.tool = PlaterTool::Orbit;
                self.scene.keep_solid = true;
                self.status = "click a triangle to paint seam".into();
                self.refresh_paint_overlay();
            }
            Message::PaintFuzzy => {
                self.paint_kind = Some(PaintKind::Fuzzy);
                self.scene.tool = PlaterTool::Orbit;
                self.scene.keep_solid = true;
                self.status = "click a triangle to paint fuzzy".into();
                self.refresh_paint_overlay();
            }
            Message::PaintClear => {
                self.paint_kind = None;
                self.scene.keep_solid = false;
                self.status = "paint mode off".into();
                self.refresh_paint_overlay();
            }
            Message::PaintBlocker(v) => self.paint_blocker = v,
            Message::BrushRadius(v) => self.brush_mm = v.clamp(0.5, 12.0),
            Message::ShowAxes(v) => {
                self.show_axes = v;
                self.save_ui_prefs();
                self.sync_gizmo();
            }
            Message::MeshletsReady { gen, meshlets } => {
                if gen == self.meshlet_gen {
                    self.scene.apply_meshlets(meshlets);
                }
            }
            Message::DisplayPacked { gen, shaded } => {
                self.pack_inflight = false;
                if gen == self.pack_gen {
                    let keep = self.scene.keep_solid;
                    self.scene.apply_shaded(*shaded);
                    self.scene.keep_solid = keep;
                    self.refresh_paint_overlay();
                    self.sync_gizmo();
                }
                if self.needs_display_pack {
                    return self.flush_display_pack();
                }
            }
            Message::PreviewMove(i) => {
                self.scene.preview_vertices = i;
                self.scene.preview_layer =
                    self.scene.toolpaths.layer_index_for_vertices(i as usize) as u32;
            }
            Message::EnableSupport(v) => self.settings.enable_support = v,
            Message::RefreshStatus => return self.refresh_monitor(),
            Message::Status(Ok(snap)) => {
                self.monitor_fetching = false;
                if self.control_bed.is_empty() && snap.machine.bed_target_c > 0.0 {
                    self.control_bed = format!("{:.0}", snap.machine.bed_target_c);
                }
                if self.control_nozzle.is_empty() && snap.machine.nozzle_target_c > 0.0 {
                    self.control_nozzle = format!("{:.0}", snap.machine.nozzle_target_c);
                }
                self.machine = monitor::retain_machine(&self.machine, snap.machine.clone());
                self.ams = monitor::retain_ams(&self.ams, snap.ams.clone());
                self.hms_lines = snap.hms_lines.clone();
                self.mqtt_status = snap.line.clone();
                self.status = snap.line.clone();
            }
            Message::Status(Err(err)) => {
                self.monitor_fetching = false;
                self.status = format!("status failed: {err}");
            }
            Message::Pause => return self.run_print_cmd(PrintCmd::Pause),
            Message::Resume => return self.run_print_cmd(PrintCmd::Resume),
            Message::Stop => {
                self.stop_confirm = true;
            }
            Message::StopCancel => {
                self.stop_confirm = false;
            }
            Message::StopConfirm => {
                self.stop_confirm = false;
                return self.run_print_cmd(PrintCmd::Stop);
            }
            Message::PrintSpeed(level) => {
                if !monitor::print_speed_active(&self.machine.gcode_state) {
                    self.push_toast("This only takes effect during printing".into());
                    return Task::none();
                }
                return self.run_print_cmd(PrintCmd::Speed(level));
            }
            Message::ChamberLight(on) => return self.run_print_cmd(PrintCmd::Light(on)),
            Message::BedSet(s) => self.control_bed = s,
            Message::NozzleSet(s) => self.control_nozzle = s,
            Message::FanSet(v) => self.control_fan = v.round().clamp(0.0, 255.0) as u8,
            Message::SendBed => {
                let temp = parse_temp_c(&self.control_bed);
                return self.run_print_cmd(PrintCmd::Bed(temp));
            }
            Message::SendNozzle => {
                let temp = parse_temp_c(&self.control_nozzle);
                return self.run_print_cmd(PrintCmd::Nozzle(temp));
            }
            Message::SendFan => {
                return self.run_print_cmd(PrintCmd::Fan {
                    index: 1,
                    speed: self.control_fan,
                });
            }
            Message::AmsLoad { ams_id, slot_id } => {
                let (old_temp, new_temp) = self.ams_filament_temps(ams_id, slot_id);
                return self.run_print_cmd(PrintCmd::AmsLoad {
                    ams_id,
                    slot_id,
                    old_temp,
                    new_temp,
                });
            }
            Message::AmsUnload { ams_id } => {
                return self.run_print_cmd(PrintCmd::AmsUnload { ams_id });
            }
            Message::AmsDryToggle(ams_id) => {
                if self.dry_ams == Some(ams_id) {
                    self.dry_ams = None;
                } else {
                    self.dry_ams = Some(ams_id);
                    let filament = self
                        .ams
                        .trays
                        .iter()
                        .find(|t| t.ams_id == ams_id)
                        .map(|t| t.filament_type.as_str())
                        .unwrap_or("PLA");
                    let (temp, hours) = monitor::drying_preset(filament);
                    if self.dry_temp.is_empty() {
                        self.dry_temp = temp.to_string();
                    }
                    if self.dry_hours.is_empty() {
                        self.dry_hours = hours.to_string();
                    }
                }
            }
            Message::AmsDryTemp(s) => self.dry_temp = s,
            Message::AmsDryHours(s) => self.dry_hours = s,
            Message::AmsDryRotate(v) => self.dry_rotate = v,
            Message::AmsDryStart(ams_id) => {
                let filament = self
                    .ams
                    .trays
                    .iter()
                    .find(|t| t.ams_id == ams_id)
                    .map(|t| t.filament_type.clone())
                    .unwrap_or_else(|| "PLA".into());
                let temp = parse_temp_c(&self.dry_temp);
                let hours = parse_temp_c(&self.dry_hours).min(48);
                return self.run_print_cmd(PrintCmd::AmsDry {
                    ams_id,
                    filament,
                    temp,
                    hours,
                    rotate: self.dry_rotate,
                });
            }
            Message::AmsDryStop(ams_id) => {
                return self.run_print_cmd(PrintCmd::AmsDryStop { ams_id });
            }
            Message::RackMove(action) => {
                self.rack_pending = Some(RackPending::Move(action));
            }
            Message::RackWarnCancel => {
                self.rack_pending = None;
            }
            Message::RackWarnConfirm => {
                let Some(pending) = self.rack_pending.take() else {
                    return Task::none();
                };
                return self.run_print_cmd(match pending {
                    RackPending::Move(action) => PrintCmd::RackMove(action),
                    RackPending::ReadAll => PrintCmd::RackRead(0xff),
                });
            }
            Message::RackReadAll => {
                self.rack_pending = Some(RackPending::ReadAll);
            }
            Message::RackConfirmAll => {
                return self.run_print_cmd(PrintCmd::RackConfirm(0xff));
            }
            Message::HmsResume => {
                let Some(cmd) = self.hms_cmd(true) else {
                    self.status = "no HMS item to resume".into();
                    return Task::none();
                };
                return self.run_print_cmd(cmd);
            }
            Message::HmsIgnore => {
                let Some(cmd) = self.hms_cmd(false) else {
                    self.status = "no HMS item to ignore".into();
                    return Task::none();
                };
                return self.run_print_cmd(cmd);
            }
            Message::ProjectBedLevel(v) => self.project_opts.bed_leveling = v,
            Message::ProjectFlowCali(v) => self.project_opts.flow_cali = v,
            Message::ProjectVibrationCali(v) => self.project_opts.vibration_cali = v,
            Message::ProjectLayerInspect(v) => self.project_opts.layer_inspect = v,
            Message::ProjectTimelapse(v) => self.project_opts.timelapse = v,
            Message::PrintControl(Ok(msg)) => {
                self.status = msg;
                return self.refresh_monitor();
            }
            Message::PrintControl(Err(err)) => self.status = format!("command failed: {err}"),
            Message::LiveMonitor(v) => {
                self.live_monitor = v;
                if v {
                    return self.refresh_monitor();
                }
            }
            Message::SendVia(v) => self.send_via = v,
            Message::ProcessProfile(name) => {
                if let Some(path) = self
                    .process_profiles
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| p.path.clone())
                {
                    match load_bbl_process(&path) {
                        Ok(s) => {
                            self.settings = s;
                            self.process_name = Some(name);
                            if let Some(slot) =
                                self.filament_slots.get(self.active_filament).cloned()
                            {
                                let label = filament::filament_pick_label(slot.source, &slot.name);
                                self.apply_filament_slot_preset(self.active_filament, &label);
                            }
                            if let Some(mac) = self.machine_name.clone() {
                                self.apply_named_profile(BblProfileKind::Machine, &mac);
                            }
                            self.sync_filament_map();
                            self.status = format!("process {path:?}");
                        }
                        Err(err) => self.status = format!("process profile: {err}"),
                    }
                }
            }
            Message::AddFilamentSlot => self.add_filament_slot(),
            Message::RemoveFilamentSlot => self.remove_filament_slot(),
            Message::SelectFilamentSlot(i) => self.select_filament_slot(i),
            Message::FilamentSlotPreset { slot, label } => {
                self.apply_filament_slot_preset(slot, &label);
            }
            Message::FilamentSlotColour { slot, colour } => {
                self.set_filament_slot_colour(slot, colour);
            }
            Message::CycleFilamentColour(slot) => {
                if let Some(current) = self.filament_slots.get(slot).map(|s| s.colour.clone()) {
                    self.set_filament_slot_colour(slot, filament::next_palette_colour(&current));
                }
            }
            Message::UserPresetName(s) => self.user_preset_name = s,
            Message::SaveUserPreset => self.save_active_as_user_preset(),
            Message::DeleteUserPreset => self.delete_active_user_preset(),
            Message::MachineProfile(name) => {
                self.apply_named_profile(BblProfileKind::Machine, &name);
            }
            Message::SelectObject(i) => {
                self.selected_object = i;
                self.selected_volume = 0;
                self.sync_gizmo();
                self.fill_xform_edits();
            }
            Message::SelectVolume(i) => self.selected_volume = i,
            Message::HideVolume(hidden) => {
                if let Some(vol) = self.selected_vol_mut() {
                    vol.hidden = hidden;
                    self.sync_scene_mesh();
                }
            }
            Message::CycleAmsMap(i) => {
                if self.settings.filament_map.is_empty() {
                    self.settings.filament_map = vec![1];
                }
                if let Some(slot) = self.settings.filament_map.get_mut(i) {
                    let max = self.ams.slot_count.max(4) as i32;
                    *slot = if *slot >= max { 0 } else { *slot + 1 };
                }
            }
            Message::RefreshHms => {
                self.status = "HMS catalog e.bambulab.com…".into();
                return Task::perform(
                    async {
                        std::thread::spawn(|| {
                            refresh_catalog(bambu_protocol::default_config_dir(), "en")
                                .map(|_| "HMS catalog cached".to_string())
                                .map_err(|err| err.to_string())
                        })
                        .join()
                        .unwrap_or_else(|_| Err("HMS thread panicked".into()))
                    },
                    Message::HmsCatalog,
                );
            }
            Message::HmsCatalog(Ok(msg)) => {
                self.status = msg;
                if !self.machine.hms.is_empty() {
                    let catalog = load_cached_catalog(bambu_protocol::default_config_dir(), "en");
                    self.hms_lines = self
                        .machine
                        .hms
                        .iter()
                        .map(|h| describe_hms(catalog.as_ref(), *h, "en"))
                        .collect();
                }
            }
            Message::HmsCatalog(Err(err)) => self.status = format!("HMS catalog: {err}"),
            Message::Calibration => {
                if self.busy {
                    self.status = "busy…".into();
                    return Task::none();
                }
                if let Some(path) = calibration_block_path() {
                    return self.begin_open(path, false, "loading calibration…");
                }
                self.status = "tests/calibration_block not found".into();
            }
            Message::SyncAms => self.sync_ams(),
            Message::FilamentPage(page) => self.filament_page = page,
            Message::FilamentMapMode(mode) => {
                self.settings.filament_map_mode = mode;
                self.apply_group_mode();
            }
            Message::AssignSlotExtruder { slot, extruder } => {
                self.settings.filament_map_mode = FilamentMapMode::Manual;
                if let Some(mapped) = self.settings.filament_map.get_mut(slot) {
                    *mapped = extruder.clamp(1, 2);
                }
            }
            Message::ParamSoluble(v) => {
                self.settings.filament_soluble = v;
                self.mark_active_support_flags();
            }
            Message::ParamSupport(v) => {
                self.settings.filament_is_support = v;
                if let Some(s) = self.filament_slots.get_mut(self.active_filament) {
                    s.is_support = v;
                }
            }
            Message::ParamPaEnable(v) => self.settings.enable_pressure_advance = v,
            Message::ParamPa(v) => self.settings.pressure_advance = v.max(0.0),
            Message::ParamRangeLow(v) => {
                self.settings.nozzle_temperature_range_low = v.round().clamp(0.0, 500.0) as u16;
            }
            Message::ParamCost(v) => self.settings.filament_cost = v.max(0.0),
            Message::ParamNotes(s) => self.settings.filament_notes = s,
            Message::ParamAdaptiveVol(v) => self.settings.filament_adaptive_volumetric_speed = v,
            Message::ParamPrime(v) => self.settings.filament_prime_volume = v.max(0.0),
            Message::ParamFlushTemp(v) => self.settings.filament_flush_temp = v.round() as i32,
            Message::ParamFlushTempFast(v) => {
                self.settings.filament_flush_temp_fast = v.round() as i32;
            }
            Message::ParamFlushVol(v) => self.settings.filament_flush_volumetric_speed = v.max(0.0),
            Message::ParamRammingVol(v) => self.settings.filament_ramming_volumetric_speed = v,
            Message::ParamRammingTravel(v) => {
                self.settings.filament_ramming_travel_time = v.max(0.0);
            }
            Message::ParamPrecool(v) => {
                self.settings.filament_pre_cooling_temperature = v.round() as i32;
            }
            Message::ParamPrintable(v) => {
                self.settings.filament_printable = if v { 1 } else { 0 };
            }
            Message::ParamStartGcode(s) => self.settings.filament_start_gcode = s,
            Message::ParamEndGcode(s) => self.settings.filament_end_gcode = s,
            Message::ParamRetract(v) => self.settings.retraction_length_mm = v.max(0.0),
            Message::ParamWipe(v) => self.settings.wipe = v,
            Message::ParamLongEc(v) => self.settings.long_retraction_when_ec = v,
            Message::CoolingFanMin(v) => {
                self.settings.fan_min_speed = v.round().clamp(0.0, 100.0) as u32;
            }
            Message::CoolingFanMax(v) => {
                self.settings.fan_max_speed = v.round().clamp(0.0, 100.0) as u32;
            }
            Message::CoolingSlowdown(v) => self.settings.slow_down_layer_time_s = v.max(0.0),
            Message::CatalogQuery(s) => self.catalog_query = s,
            Message::CatalogAdd(id) => self.add_catalog_spool(&id),
            Message::ShowBblPresets(v) => self.show_bbl_presets = v,
            Message::ShowArchived(v) => self.show_archived = v,
            Message::InventorySearch(s) => self.spool_search = s,
            Message::InventoryNew => {
                self.draft_spool = FilamentSpool::default();
                self.draft_location.clear();
                self.draft_archived = false;
                self.selected_spool = None;
            }
            Message::InventorySelect(i) => {
                if let Some(spool) = self.inventory.spools.get(i) {
                    self.selected_spool = Some(i);
                    self.draft_spool = self.inventory.flatten(spool);
                    self.draft_location = spool.location.clone();
                    self.draft_archived = spool.archived;
                }
            }
            Message::InventorySave => self.save_draft_spool(),
            Message::InventoryDelete => self.delete_selected_spool(),
            Message::InventoryPull => {
                if !self.begin_work("pulling cloud filaments…") {
                    return Task::none();
                }
                return offload(pull_cloud_filaments, Message::InventoryPulled);
            }
            Message::InventoryPush => {
                if !self.begin_work("pushing filaments…") {
                    return Task::none();
                }
                let spools: Vec<FilamentSpool> = self
                    .inventory
                    .spools
                    .iter()
                    .map(|s| self.inventory.flatten(s))
                    .collect();
                return offload(
                    move || push_cloud_filaments(&spools),
                    Message::InventoryPushed,
                );
            }
            Message::InventoryPulled(result) => {
                self.busy = false;
                match result {
                    Ok(list) => {
                        self.merge_pulled_spools(list);
                        self.status = format!("{} spool(s)", self.inventory.spools.len());
                    }
                    Err(err) => self.status = format!("pull failed: {err}"),
                }
            }
            Message::InventoryPushed(result) => {
                self.busy = false;
                match result {
                    Ok(msg) => self.status = msg,
                    Err(err) => self.status = format!("push failed: {err}"),
                }
            }
            Message::BindSpoolToSlot => self.bind_spool_to_active_slot(),
            Message::SpoolBrand(s) => self.draft_spool.brand = s,
            Message::SpoolMaterial(s) => self.draft_spool.material_type = s,
            Message::SpoolSeries(s) => self.draft_spool.series = s,
            Message::SpoolColor(s) => self.draft_spool.color_code = s,
            Message::SpoolFilamentId(s) => self.draft_spool.filament_id = s,
            Message::SpoolNote(s) => self.draft_spool.note = s,
            Message::SpoolLocation(s) => self.draft_location = s,
            Message::SpoolArchived(v) => self.draft_archived = v,
            Message::SpoolUseGrams(s) => self.draft_use = s,
            Message::SpoolMeasureGross(s) => self.draft_measure = s,
            Message::ApplySpoolUse => self.apply_spool_use(),
            Message::ApplySpoolMeasure => self.apply_spool_measure(),
            Message::PlaterTool(tool) => {
                self.apply_plater_tool(tool);
                self.refresh_paint_overlay();
                self.sync_gizmo();
            }
            Message::Arrange => self.arrange_plate(),
            Message::AutoOrient => self.auto_orient_selected(),
            Message::Mirror(axis) => self.mirror_selected(axis),
            Message::Rotate90 => self.rotate_selected_90(),
            Message::CoordSpace(space) => self.set_coord_space(space),
            Message::UniformScale(v) => self.set_uniform_scale(v),
            Message::XformDraft { field, text } => self.set_xform_draft(field, text),
            Message::XformCommit(field) => self.commit_xform_field(field),
            Message::DropToBed => self.drop_selected_to_bed(),
            Message::ResetRotation => self.reset_selected_rotation(),
            Message::ResetScale => self.reset_selected_scale(),
            Message::ProcessTab(tab) => self.quality_tab = tab,
            Message::ProcessSearch(q) => self.process_search = q,
            Message::ProcessObjects(v) => self.process_objects = v,
            Message::TogglePrinterSection => self.printer_open = !self.printer_open,
            Message::ToggleFilamentSection => self.filament_open = !self.filament_open,
            Message::ToggleProcessSection => self.process_open = !self.process_open,
            Message::Seam(pos) => self.settings.seam = pos,
            Message::SeamAway(v) => self.settings.seam_placement_away_from_overhangs = v,
            Message::ScarfConditional(v) => self.settings.seam_slope_conditional = v,
            Message::ScarfEntireLoop(v) => self.settings.seam_slope_entire_loop = v,
            Message::ScarfInnerWalls(v) => self.settings.seam_slope_inner_walls = v,
            Message::OverrideScarf(v) => self.settings.override_filament_scarf_seam_setting = v,
            Message::FirstLayerHeight(v) => {
                self.settings.first_layer_height_mm = v.clamp(0.08, 0.4);
            }
            Message::PreciseOuterWall(v) => self.settings.precise_outer_wall = v,
            Message::OnlyOneWallFirst(v) => self.settings.only_one_wall_first_layer = v,
            Message::OnlyOneWallTop(v) => {
                self.settings.top_one_wall = if v {
                    TopOneWallType::AllTop
                } else {
                    TopOneWallType::None
                };
            }
            Message::PlatePrev => {
                let n = self.plate_count();
                self.set_plate(self.plate.checked_sub(1).unwrap_or(n.saturating_sub(1)));
            }
            Message::PlateNext => {
                let n = self.plate_count();
                self.set_plate(if n == 0 { 0 } else { (self.plate + 1) % n });
            }
            Message::Toast(msg) => self.push_toast(msg),
        }
        self.flush_display_pack()
    }

    pub fn theme(&self) -> Theme {
        theme::studio()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let body: Element<'_, Message> = match self.workspace {
            Workspace::Home => self.home_page(),
            Workspace::Device => self.device_page(),
            Workspace::Filament => self.inventory_page(),
            Workspace::Project => self.stub_page(
                "Project",
                "Project files and plate notes — stub pane (no new backend this pass).",
            ),
            Workspace::Calibration => self.stub_page(
                "Calibration",
                "Calibration wizard lives in C++ Studio — stub pane here.",
            ),
            Workspace::Prepare | Workspace::Preview => {
                let stage = self.viewport_stage();
                if self.sidebar_collapsed {
                    stage
                } else {
                    let sidebar = container(match self.workspace {
                        Workspace::Preview => self.preview_sidebar(),
                        _ => self.prepare_sidebar(),
                    })
                    .style(|_| theme::sidebar_pane())
                    .width(SIDEBAR_WIDTH)
                    .height(Fill);
                    row![stage, sidebar].height(Fill).into()
                }
            }
        };
        let chrome: Element<'_, Message> = column![
            container(self.top_bar()).style(|_| theme::header_bar()),
            container(body).height(Fill),
        ]
        .height(Fill)
        .into();
        match self.header_menu_overlay() {
            Some(overlay) => stack![chrome, overlay].width(Fill).height(Fill).into(),
            None => chrome,
        }
    }

    pub fn slice_cpu_blocking(&mut self) {
        let settings = self.settings.clone();
        let mesh = self.scene.mesh.clone();
        match bambu_slicer::slice_mesh(&mesh, &settings) {
            Ok(result) => {
                let _ = self.finish_slice(&settings, result, "cpu");
            }
            Err(err) => self.status = format!("cpu slice failed: {err}"),
        }
    }

    pub(crate) fn plate_count(&self) -> usize {
        self.model
            .as_ref()
            .map(|m| m.plates.len().max(1))
            .unwrap_or(1)
    }

    pub(crate) fn plate_label(&self) -> String {
        self.model
            .as_ref()
            .and_then(|m| m.plates.get(self.plate))
            .map(|p| p.name.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("Plate {}", self.plate + 1))
    }

    pub(crate) fn show_left_nozzle_only(&self) -> bool {
        self.settings.nozzle_count() > 1
    }

    fn set_plate(&mut self, i: usize) {
        let n = self.plate_count();
        self.plate = i.min(n.saturating_sub(1));
        self.sync_scene_mesh();
        self.status = format!("plate {}", self.plate + 1);
    }

    fn push_toast(&mut self, msg: String) {
        self.status = msg.clone();
        self.toasts.push(msg);
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }
    fn slice_current(&mut self) -> Task<Message> {
        if !self.begin_work("slicing…") {
            return Task::none();
        }
        self.settings.print_sequence = if self.by_object {
            String::from("by object")
        } else {
            String::from("by layer")
        };
        let job = self.build_slice_job();
        offload(move || run_slice_job(job), Message::Sliced)
    }

    fn build_slice_job(&self) -> SliceJob {
        let settings = self.settings.clone();
        if let Some(model) = &self.model {
            let indices: Vec<usize> = model.plate_object_indices(self.plate);
            if settings.print_sequence_by_object() && indices.len() > 1 {
                let objects = indices
                    .iter()
                    .filter_map(|&i| model.objects.get(i).map(|obj| obj.volumes_or_mesh()))
                    .collect();
                return SliceJob {
                    settings,
                    mesh: self.scene.mesh.clone(),
                    volumes: None,
                    objects: Some(objects),
                };
            }
            let vols = model.world_volumes_for_plate(self.plate);
            if vols.len() > 1
                || vols
                    .iter()
                    .any(bambu_model::ModelVolume::needs_volume_slice)
            {
                return SliceJob {
                    settings,
                    mesh: self.scene.mesh.clone(),
                    volumes: Some(vols),
                    objects: None,
                };
            }
        }
        SliceJob {
            settings,
            mesh: self.scene.mesh.clone(),
            volumes: None,
            objects: None,
        }
    }

    fn apply_slice_outcome(&mut self, outcome: SliceOutcome) -> Task<Message> {
        match outcome {
            SliceOutcome::Single {
                result,
                backend,
                settings,
            } => self.finish_slice(&settings, result, backend.as_str()),
            SliceOutcome::Objects { results, settings } => self.finish_objects(&settings, results),
        }
    }

    fn finish_slice(
        &mut self,
        settings: &SliceSettings,
        result: bambu_slicer::SliceResult,
        backend: &str,
    ) -> Task<Message> {
        match write_gcode(settings, &result) {
            Ok(gcode) => {
                self.estimated_seconds = parse_gcode(&gcode).estimated_seconds;
                self.last_gcode = Some(gcode);
            }
            Err(err) => {
                self.status = format!("gcode failed: {err}");
                return Task::none();
            }
        }
        let support = result
            .layers
            .iter()
            .filter(|l| !l.support.is_empty() || !l.support_interface.is_empty())
            .count();
        self.status = format!(
            "sliced {} layers @ {}mm ({backend}; skirt {} · support {})",
            result.layers.len(),
            settings.layer_height_mm,
            result.layers.first().map(|l| l.skirt.len()).unwrap_or(0),
            support
        );
        self.scene
            .set_toolpaths(ToolpathBuffer::from_slice(&result));
        self.workspace = Workspace::Preview;
        self.sync_keep_solid();
        Task::none()
    }

    fn finish_objects(
        &mut self,
        settings: &SliceSettings,
        results: Vec<bambu_slicer::SliceResult>,
    ) -> Task<Message> {
        match write_gcode_for_objects(settings, &results) {
            Ok(gcode) => {
                self.estimated_seconds = parse_gcode(&gcode).estimated_seconds;
                self.last_gcode = Some(gcode);
            }
            Err(err) => {
                self.status = format!("gcode failed: {err}");
                return Task::none();
            }
        }
        let mut merged = bambu_slicer::SliceResult { layers: Vec::new() };
        for result in &results {
            merged.layers.extend(result.layers.iter().cloned());
        }
        self.status = format!(
            "sliced {} objects · {} layers (by object)",
            results.len(),
            merged.layers.len()
        );
        self.scene
            .set_toolpaths(ToolpathBuffer::from_slice(&merged));
        self.workspace = Workspace::Preview;
        self.sync_keep_solid();
        Task::none()
    }

    fn paint_pick(&mut self, ndc_x: f32, ndc_y: f32, aspect: f32) {
        let Some(kind) = self.paint_kind else {
            return;
        };
        let (origin, dir) = self.scene.camera.ray_from_ndc(ndc_x, ndc_y, aspect);
        let Some(hit) = self.scene.pick_triangle(origin, dir) else {
            self.status = "no triangle under cursor".into();
            return;
        };
        if self.model.is_none() {
            self.model = Some(Arc::new(Model::from_mesh(
                "viewport",
                self.scene.mesh.clone(),
            )));
        }
        let idx = self.scene.mesh.indices[hit];
        let [a, b, c] = self.scene.mesh.triangle(idx);
        let centroid = (a + b + c) / 3.0;
        let tris = if self.brush_mm > 0.51 {
            let mut near = self.scene.triangles_near(centroid, self.brush_mm);
            if !near.contains(&hit) {
                near.push(hit);
            }
            near
        } else {
            vec![hit]
        };
        let paint = if self.paint_blocker {
            TrianglePaint::Blocker
        } else {
            TrianglePaint::Enforcer
        };
        let mut painted = 0usize;
        for tri in tris {
            if self.paint_one(kind, tri, paint) {
                painted += 1;
            }
        }
        self.refresh_paint_overlay();
        self.status = if painted > 0 {
            format!("painted {kind:?} x{painted} — Slice to apply")
        } else {
            format!("triangle {hit} not on a model volume")
        };
    }

    fn paint_one(&mut self, kind: PaintKind, tri: usize, paint: TrianglePaint) -> bool {
        let Some(model) = self.model_mut() else {
            return false;
        };
        let mut remaining = tri;
        for obj in &mut model.objects {
            if obj.volumes.is_empty() {
                obj.volumes = obj.volumes_or_mesh();
            }
            for vol in &mut obj.volumes {
                let n = vol.mesh.indices.len();
                if remaining >= n {
                    remaining -= n;
                    continue;
                }
                match kind {
                    PaintKind::Support => {
                        if vol.triangle_support.len() < n {
                            vol.triangle_support = vec![TrianglePaint::None; n];
                        }
                        vol.triangle_support[remaining] = paint;
                    }
                    PaintKind::Seam => {
                        if vol.triangle_seam.len() < n {
                            vol.triangle_seam = vec![TrianglePaint::None; n];
                        }
                        vol.triangle_seam[remaining] = paint;
                    }
                    PaintKind::Fuzzy => {
                        if vol.triangle_fuzzy_skin.len() < n {
                            vol.triangle_fuzzy_skin = vec![TrianglePaint::None; n];
                        }
                        vol.triangle_fuzzy_skin[remaining] = paint;
                    }
                }
                return true;
            }
        }
        false
    }

    fn plate_buttons(&self) -> Element<'_, Message> {
        let n = self
            .model
            .as_ref()
            .map(|m| m.plates.len().max(1))
            .unwrap_or(1);
        let mut r = row![];
        for i in 0..n {
            let name = self
                .model
                .as_ref()
                .and_then(|m| m.plates.get(i))
                .map(|p| p.name.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("{}", i + 1));
            r = r.push(
                button(text(name).size(12))
                    .padding([3, 8])
                    .on_press(Message::Plate(i)),
            );
        }
        r.spacing(4).into()
    }

    fn object_panel(&self) -> Element<'_, Message> {
        let Some(model) = &self.model else {
            return text("cube").size(12).into();
        };
        if model.objects.is_empty() {
            return text("—").size(12).into();
        }
        let mut col = column![];
        for (i, obj) in model.objects.iter().enumerate() {
            let name = if obj.name.is_empty() {
                format!("object {}", i + 1)
            } else {
                obj.name.clone()
            };
            col = col.push(tree_chip(
                name,
                i == self.selected_object,
                Message::SelectObject(i),
            ));
            if i != self.selected_object {
                continue;
            }
            for (vi, vol) in obj.volumes.iter().enumerate() {
                let extruder = vol
                    .config
                    .get("extruder")
                    .cloned()
                    .unwrap_or_else(|| "—".into());
                let vol_name = if vol.name.is_empty() {
                    format!("v{}", vi + 1)
                } else {
                    vol.name.clone()
                };
                let hidden = if vol.hidden { " (hidden)" } else { "" };
                col = col.push(tree_chip(
                    format!(
                        "  · {} {vol_name} ext {extruder}{hidden}",
                        vol.volume_type.as_str()
                    ),
                    vi == self.selected_volume,
                    Message::SelectVolume(vi),
                ));
            }
            if let Some(vol) = obj.volumes.get(self.selected_volume) {
                col = col.push(
                    checkbox(vol.hidden)
                        .label("Hide volume")
                        .on_toggle(Message::HideVolume)
                        .style(theme::tick),
                );
            }
        }
        col.spacing(4).into()
    }

    fn apply_named_profile(&mut self, kind: BblProfileKind, name: &str) {
        let path = match kind {
            BblProfileKind::Process => self
                .process_profiles
                .iter()
                .find(|p| p.name == name)
                .map(|p| p.path.clone()),
            BblProfileKind::Filament => self.filament_path(FilamentSource::System, name),
            BblProfileKind::Machine => self
                .machine_profiles
                .iter()
                .find(|p| p.name == name)
                .map(|p| p.path.clone()),
        };
        let Some(path) = path else {
            self.status = format!("no {name} profile");
            return;
        };
        match overlay_bbl_profile(&mut self.settings, &path) {
            Ok(()) => {
                match kind {
                    BblProfileKind::Process => self.process_name = Some(name.to_string()),
                    BblProfileKind::Filament => {
                        self.filament_name = Some(name.to_string());
                        if let Some(slot) = self.filament_slots.get_mut(self.active_filament) {
                            slot.name = name.to_string();
                            slot.source = FilamentSource::System;
                            if !self.settings.filament_colour.is_empty() {
                                slot.colour = self.settings.filament_colour.clone();
                            }
                        }
                    }
                    BblProfileKind::Machine => {
                        self.machine_name = Some(name.to_string());
                        self.apply_bed_from_settings();
                    }
                }
                self.status = format!("overlay {}", path.display());
            }
            Err(err) => self.status = format!("profile: {err}"),
        }
    }

    fn begin_work(&mut self, status: &str) -> bool {
        if self.busy {
            self.status = "busy…".into();
            false
        } else {
            self.busy = true;
            self.status = status.into();
            true
        }
    }

    fn begin_open(&mut self, path: PathBuf, apply_settings: bool, status: &str) -> Task<Message> {
        self.file_menu_open = false;
        self.workspace = Workspace::Prepare;
        self.sync_keep_solid();
        if apply_settings {
            self.project_file = Some(path.clone());
        }
        if !self.begin_work(status) {
            return Task::none();
        }
        self.load_gen = self.load_gen.wrapping_add(1);
        let gen = self.load_gen;
        self.load_progress = Some(LoadProgress {
            label: LoadStage::OpenArchive.label(),
            fraction: LoadStage::OpenArchive.fraction(),
        });
        self.scene.set_solids(Vec::new());
        let job = LoadJob {
            gen,
            path,
            apply_settings,
            bed: self.scene.bed.clone(),
        };
        Task::stream(load_model_stream(job))
    }

    fn sync_keep_solid(&mut self) {
        self.scene.keep_solid = self.workspace != Workspace::Preview
            || self.paint_kind.is_some()
            || !self.scene.paint_overlay.is_empty();
    }

    fn remember_recent(&mut self, path: PathBuf) {
        self.recent_models.retain(|p| p != &path);
        self.recent_models.insert(0, path);
        self.recent_models.truncate(8);
        self.save_recents();
        self.refresh_recent_thumbs();
    }

    fn recents_path() -> PathBuf {
        bambu_protocol::default_config_dir().join("recent_projects.json")
    }

    fn load_recents(&mut self) {
        let Ok(bytes) = std::fs::read(Self::recents_path()) else {
            return;
        };
        if let Ok(paths) = serde_json::from_slice::<Vec<PathBuf>>(&bytes) {
            self.recent_models = paths.into_iter().filter(|p| p.exists()).take(8).collect();
        }
        self.refresh_recent_thumbs();
    }

    fn refresh_recent_thumbs(&mut self) {
        let mut keep = HashMap::new();
        for path in &self.recent_models {
            let mtime = file_mtime(path);
            let key = (path.clone(), mtime);
            if let Some(handle) = self.recent_thumbs.remove(&key) {
                keep.insert(key, handle);
                continue;
            }
            if !is_project_3mf(path) {
                continue;
            }
            if let Ok(Some(png)) = read_3mf_thumbnail(path) {
                if let Some(handle) = png_to_handle(&png) {
                    keep.insert(key, handle);
                }
            }
        }
        self.recent_thumbs = keep;
    }

    fn recent_thumb(&self, path: &PathBuf) -> Option<iced::widget::image::Handle> {
        let mtime = file_mtime(path);
        self.recent_thumbs.get(&(path.clone(), mtime)).cloned()
    }

    fn ui_prefs_path() -> PathBuf {
        bambu_protocol::default_config_dir().join("ui_prefs.json")
    }

    fn load_ui_prefs(&mut self) {
        let Ok(bytes) = std::fs::read(Self::ui_prefs_path()) else {
            return;
        };
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            if let Some(show) = v.get("show_axes").and_then(|x| x.as_bool()) {
                self.show_axes = show;
            }
        }
    }

    fn save_ui_prefs(&self) {
        let path = Self::ui_prefs_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let json = serde_json::json!({ "show_axes": self.show_axes });
        if let Ok(bytes) = serde_json::to_vec_pretty(&json) {
            let _ = std::fs::write(path, bytes);
        }
    }

    fn save_recents(&self) {
        let path = Self::recents_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(&self.recent_models) {
            let _ = std::fs::write(path, json);
        }
    }

    fn new_project(&mut self) -> Task<Message> {
        self.model = Some(Arc::new(bambu_model::Model {
            objects: Vec::new(),
            plates: vec![bambu_model::PartPlate {
                name: "Plate 1".into(),
                object_indices: Vec::new(),
                locked: false,
            }],
            settings: None,
        }));
        self.project_file = None;
        self.last_gcode = None;
        self.estimated_seconds = None;
        self.plate = 0;
        self.selected_object = 0;
        self.selected_volume = 0;
        self.scene.set_mesh(bambu_geom::TriangleMesh {
            vertices: Vec::new(),
            indices: Vec::new(),
        });
        self.workspace = Workspace::Prepare;
        self.sync_keep_solid();
        self.load_progress = None;
        self.status = "new project".into();
        Task::none()
    }

    fn save_project(&self, save_as: bool) -> Task<Message> {
        if self.model.is_none() {
            return Task::none();
        }
        if !save_as {
            if let Some(path) = self.project_file.clone() {
                return self.write_project_to(path);
            }
        }
        let suggested = self
            .project_file
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("project.3mf")
            .to_string();
        Task::perform(pick_save_project_path(suggested), Message::ProjectPicked)
    }

    fn write_project_to(&self, path: PathBuf) -> Task<Message> {
        let Some(model) = self.model.clone() else {
            return Task::none();
        };
        offload(
            move || {
                write_model_3mf(&path, model.as_ref()).map_err(|e| e.to_string())?;
                Ok(path)
            },
            Message::ProjectSaved,
        )
    }

    fn apply_loaded_model(&mut self, loaded: LoadedModel) -> Task<Message> {
        self.remember_recent(loaded.path.clone());
        self.workspace = Workspace::Prepare;
        self.sync_keep_solid();
        if loaded.apply_settings {
            if let Some(s) = loaded.model.settings.clone() {
                self.settings = s;
            }
        }
        self.plate = 0;
        if loaded.apply_settings {
            self.apply_bed_from_settings();
        }
        let tris = loaded.shaded.triangle_count();
        self.object_aabbs = loaded.object_aabbs;
        self.model = Some(Arc::new(loaded.model));
        self.selected_object = 0;
        self.selected_volume = 0;
        let keep = self.scene.keep_solid;
        self.scene.apply_shaded(loaded.shaded);
        self.scene.keep_solid = keep;
        self.needs_display_pack = false;
        self.scene.frame_contents();
        self.refresh_paint_overlay();
        self.sync_gizmo();
        self.fill_xform_edits();
        let secs = loaded.load_ms as f64 / 1000.0;
        self.status = if loaded.apply_settings {
            format!(
                "loaded {} in {secs:.2}s ({} triangles, {} plates)",
                loaded.label,
                tris,
                self.model
                    .as_ref()
                    .map(|m| m.plates.len().max(1))
                    .unwrap_or(1)
            )
        } else {
            format!(
                "calibration block ({} objects) — Slice with current settings",
                self.model.as_ref().map(|m| m.objects.len()).unwrap_or(0)
            )
        };
        self.sync_keep_solid();
        Task::none()
    }

    fn rewrite_filament_dir() -> PathBuf {
        bambu_protocol::default_config_dir().join("filament")
    }

    fn reload_user_filaments(&mut self) {
        self.user_filaments = list_filament_json_dir(Self::rewrite_filament_dir());
        self.studio_filaments = list_studio_user_filaments();
    }

    fn empty_slot() -> FilamentSlot {
        FilamentSlot {
            name: String::from("Generic PLA"),
            colour: String::from("#FFFFFFFF"),
            source: FilamentSource::System,
            filament_id: String::new(),
            filament_type: String::from("PLA"),
            is_support: false,
            ams_id: None,
            slot_id: None,
            external_id: String::new(),
            spool_id: String::new(),
        }
    }

    fn init_filament_slots(&mut self) {
        self.filament_slots = vec![Self::empty_slot()];
        self.active_filament = 0;
        if let Some(spool) = self.inventory.spools.first() {
            self.apply_inventory_spool(0, &spool.id.clone());
        } else if let Some(sku) = self
            .catalog
            .by_external_id("bambulab_pla_jadewhite_1000_175_n")
            .cloned()
            .or_else(|| self.catalog.search("PLA", 1).into_iter().next().cloned())
        {
            self.apply_catalog_sku(0, &sku);
        } else {
            let (name, source) = default_filament_pick(&self.filament_profiles);
            let label = filament::filament_pick_label(source, &name);
            self.apply_filament_slot_preset(0, &label);
        }
        self.sync_filament_map();
    }

    fn filament_path(&self, source: FilamentSource, name: &str) -> Option<PathBuf> {
        let list = match source {
            FilamentSource::System => &self.filament_profiles,
            FilamentSource::User => &self.user_filaments,
            FilamentSource::Studio => &self.studio_filaments,
            FilamentSource::Catalog | FilamentSource::Inventory => return None,
        };
        list.iter()
            .find(|p| p.name == name)
            .map(|p| p.path.clone())
            .or_else(|| {
                self.filament_profiles
                    .iter()
                    .chain(self.user_filaments.iter())
                    .chain(self.studio_filaments.iter())
                    .find(|p| p.name == name)
                    .map(|p| p.path.clone())
            })
    }

    fn generic_bbl_path(&self, material: &str) -> Option<PathBuf> {
        resolve_ams_filament("", material, &self.filament_profiles).map(|p| p.path)
    }

    fn sku_from_inventory_filament(&self, filament_id: &str) -> Option<CatalogFilament> {
        let f = self.inventory.filament(filament_id)?;
        let vendor = self
            .inventory
            .vendor(&f.vendor_id)
            .map(|v| v.name.clone())
            .unwrap_or_default();
        Some(CatalogFilament {
            external_id: f.external_id.clone(),
            manufacturer: vendor,
            name: f.name.clone(),
            material: f.material.clone(),
            density: f.density,
            diameter: f.diameter,
            net_weight: f.weight,
            spool_weight: f.spool_weight,
            color_name: f.color_name.clone(),
            color_hex: f.color_hex.clone(),
            extruder_temp: f.extruder_temp,
            bed_temp: f.bed_temp,
        })
    }

    fn apply_catalog_sku(&mut self, slot: usize, sku: &CatalogFilament) {
        let generic = self.generic_bbl_path(&sku.material);
        if let Err(err) = apply_sku_with_generic_base(&mut self.settings, sku, generic.as_deref()) {
            self.status = format!("catalog overlay: {err}");
            return;
        }
        if let Some(s) = self.filament_slots.get_mut(slot) {
            s.name = sku.name.clone();
            s.source = FilamentSource::Catalog;
            s.colour = slot_colour_hex(&sku.color_hex);
            s.filament_type = sku.material.clone();
            s.external_id = sku.external_id.clone();
            s.spool_id.clear();
            s.is_support = self.settings.filament_is_support;
            if s.filament_id.is_empty() {
                s.filament_id = self.settings.filament_id.clone();
            }
        }
        self.settings.filament_colour = slot_colour_hex(&sku.color_hex);
        self.filament_name = Some(sku.name.clone());
        self.active_filament = slot.min(self.filament_slots.len().saturating_sub(1));
        self.status = format!("catalog {}", sku.external_id);
    }

    fn apply_inventory_spool(&mut self, slot: usize, spool_id: &str) {
        let Some(spool) = self.inventory.spool(spool_id).cloned() else {
            self.status = format!("no spool {spool_id}");
            return;
        };
        let Some(sku) = self.sku_from_inventory_filament(&spool.filament_id) else {
            self.status = format!("spool {spool_id} has no filament");
            return;
        };
        self.apply_catalog_sku(slot, &sku);
        if let Some(s) = self.filament_slots.get_mut(slot) {
            s.source = FilamentSource::Inventory;
            s.spool_id = spool.id.clone();
            if let Some(f) = self.inventory.filament(&spool.filament_id) {
                if !f.bambu_filament_id.is_empty() {
                    s.filament_id = f.bambu_filament_id.clone();
                    self.settings.filament_id = f.bambu_filament_id.clone();
                }
            }
        }
        if spool.ams_id >= 0 {
            if let Some(s) = self.filament_slots.get_mut(slot) {
                s.ams_id = Some(spool.ams_id as u8);
            }
        }
        if spool.slot_id >= 0 {
            if let Some(s) = self.filament_slots.get_mut(slot) {
                s.slot_id = Some(spool.slot_id as u8);
            }
        }
        self.status = format!("inventory spool {}", spool.id);
    }

    fn sync_filament_map(&mut self) {
        let n = self.filament_slots.len().max(1);
        while self.settings.filament_map.len() < n {
            self.settings.filament_map.push(1);
        }
        self.settings.filament_map.truncate(n);
        self.settings.filament_count = n;
    }

    fn add_filament_slot(&mut self) {
        let slot = self
            .filament_slots
            .last()
            .cloned()
            .unwrap_or_else(Self::empty_slot);
        self.filament_slots.push(slot);
        self.sync_filament_map();
        self.apply_group_mode();
        self.status = format!("{} filament slot(s)", self.filament_slots.len());
    }

    fn remove_filament_slot(&mut self) {
        if self.filament_slots.len() <= 1 {
            self.status = "keep at least one filament slot".into();
            return;
        }
        self.filament_slots.pop();
        if self.active_filament >= self.filament_slots.len() {
            self.select_filament_slot(self.filament_slots.len() - 1);
        }
        self.sync_filament_map();
        self.apply_group_mode();
        self.status = format!("{} filament slot(s)", self.filament_slots.len());
    }

    fn select_filament_slot(&mut self, i: usize) {
        let Some(slot) = self.filament_slots.get(i).cloned() else {
            return;
        };
        self.active_filament = i;
        match slot.source {
            FilamentSource::Catalog => {
                if let Some(sku) = self.catalog.by_external_id(&slot.external_id).cloned() {
                    self.apply_catalog_sku(i, &sku);
                }
            }
            FilamentSource::Inventory => {
                self.apply_inventory_spool(i, &slot.spool_id);
            }
            _ => {
                if let Some(path) = self.filament_path(slot.source, &slot.name) {
                    if overlay_bbl_profile(&mut self.settings, &path).is_ok() {
                        self.filament_name = Some(slot.name.clone());
                        if slot.colour.is_empty() {
                            if let Some(s) = self.filament_slots.get_mut(i) {
                                if !self.settings.filament_colour.is_empty() {
                                    s.colour = self.settings.filament_colour.clone();
                                }
                            }
                        } else {
                            self.settings.filament_colour = slot.colour.clone();
                        }
                    }
                }
            }
        }
        self.sync_filament_map();
    }

    fn apply_filament_slot_preset(&mut self, slot: usize, label: &str) {
        let Some((source, name)) = filament::parse_filament_pick(label) else {
            self.status = format!("unknown preset {label}");
            return;
        };
        match source {
            FilamentSource::Catalog => {
                if let Some(sku) = self.catalog.by_external_id(&name).cloned() {
                    self.apply_catalog_sku(slot, &sku);
                } else {
                    self.status = format!("no catalog {name}");
                }
                return;
            }
            FilamentSource::Inventory => {
                self.apply_inventory_spool(slot, &name);
                return;
            }
            _ => {}
        }
        let Some(path) = self.filament_path(source, &name) else {
            self.status = format!("no {name} profile");
            return;
        };
        self.active_filament = slot.min(self.filament_slots.len().saturating_sub(1));
        match overlay_bbl_profile(&mut self.settings, &path) {
            Ok(()) => {
                let colour = if self.settings.filament_colour.is_empty() {
                    self.filament_slots
                        .get(slot)
                        .map(|s| slot_colour_hex(&s.colour))
                        .unwrap_or_else(|| String::from("#FFFFFFFF"))
                } else {
                    slot_colour_hex(&self.settings.filament_colour)
                };
                if let Some(s) = self.filament_slots.get_mut(slot) {
                    s.name = name.clone();
                    s.source = source;
                    s.colour = colour.clone();
                    s.filament_id = self.settings.filament_id.clone();
                    s.filament_type = self.settings.filament_type.clone();
                    s.is_support = self.settings.filament_is_support;
                    s.external_id.clear();
                    s.spool_id.clear();
                    if s.filament_id.is_empty() {
                        s.filament_id = profile_filament_id(&path);
                    }
                }
                self.settings.filament_colour = colour;
                self.filament_name = Some(name);
                self.status = format!("overlay {}", path.display());
            }
            Err(err) => self.status = format!("profile: {err}"),
        }
    }

    fn set_filament_slot_colour(&mut self, slot: usize, colour: String) {
        let colour = slot_colour_hex(&colour);
        if let Some(s) = self.filament_slots.get_mut(slot) {
            s.colour = colour.clone();
        }
        if slot == self.active_filament {
            self.settings.filament_colour = colour;
        }
    }

    fn save_active_as_user_preset(&mut self) {
        let name = self.user_preset_name.trim().to_string();
        if name.is_empty() {
            self.status = "name the user preset first".into();
            return;
        }
        let Some(slot) = self.filament_slots.get(self.active_filament).cloned() else {
            return;
        };
        let Some(src) = self.filament_path(slot.source, &slot.name) else {
            self.status = "select a system or user base first".into();
            return;
        };
        match clone_filament_as_user(&src, Self::rewrite_filament_dir(), &name) {
            Ok(path) => {
                if !slot.colour.is_empty() {
                    let _ = patch_filament_colour(&path, &slot.colour);
                }
                let _ = patch_user_filament_settings(&path, &self.settings);
                self.reload_user_filaments();
                self.user_preset_name.clear();
                let label = filament::filament_pick_label(FilamentSource::User, &name);
                self.apply_filament_slot_preset(self.active_filament, &label);
                self.status = format!("saved {}", path.display());
            }
            Err(err) => self.status = format!("save user filament: {err}"),
        }
    }

    fn delete_active_user_preset(&mut self) {
        let Some(slot) = self.filament_slots.get(self.active_filament).cloned() else {
            return;
        };
        if slot.source != FilamentSource::User {
            self.status = "only rewrite user presets can be deleted".into();
            return;
        }
        match delete_user_filament(Self::rewrite_filament_dir(), &slot.name) {
            Ok(()) => {
                self.reload_user_filaments();
                let (name, source) = default_filament_pick(&self.filament_profiles);
                let label = filament::filament_pick_label(source, &name);
                self.apply_filament_slot_preset(self.active_filament, &label);
                self.status = format!("deleted user preset {}", slot.name);
            }
            Err(err) => self.status = format!("delete user filament: {err}"),
        }
    }

    fn mark_active_support_flags(&mut self) {
        if let Some(s) = self.filament_slots.get_mut(self.active_filament) {
            s.is_support = self.settings.filament_is_support;
        }
    }

    fn group_slots(&self) -> Vec<GroupSlot> {
        self.filament_slots
            .iter()
            .map(|s| GroupSlot {
                filament_id: s.filament_id.clone(),
                colour: s.colour.clone(),
                filament_type: s.filament_type.clone(),
                is_support: s.is_support,
            })
            .collect()
    }

    fn group_trays(&self) -> Vec<GroupTray> {
        self.ams
            .trays
            .iter()
            .cloned()
            .chain(self.ams.vt_tray.clone())
            .map(|t| GroupTray {
                ams_id: t.ams_id,
                tray_info_idx: t.tray_info_idx,
                filament_type: t.filament_type,
                color: t.color,
            })
            .collect()
    }

    fn apply_group_mode(&mut self) {
        let n = self.filament_slots.len().max(1);
        self.settings.filament_count = n;
        if self.settings.filament_map_mode == FilamentMapMode::Manual {
            self.sync_filament_map();
            return;
        }
        self.settings.filament_map = compute_filament_map(
            self.settings.filament_map_mode,
            n,
            self.settings.nozzle_count(),
            &self.settings.flush_volumes_mm3,
            &self.group_slots(),
            &self.group_trays(),
        );
    }

    fn sync_ams(&mut self) {
        let mut trays = self.ams.trays.clone();
        if let Some(vt) = &self.ams.vt_tray {
            trays.push(vt.clone());
        }
        trays.retain(|t| {
            !t.filament_type.is_empty() || !t.tray_info_idx.is_empty() || !t.color.is_empty()
        });
        if trays.is_empty() {
            self.status = "no AMS trays — Refresh status first".into();
            return;
        }
        let mut slots = Vec::new();
        for tray in trays {
            let resolved = resolve_ams_filament(
                &tray.tray_info_idx,
                &tray.filament_type,
                &self.filament_profiles,
            );
            let (name, source, path) = match resolved {
                Some(p) => (p.name, FilamentSource::System, Some(p.path)),
                None => (
                    if tray.filament_type.is_empty() {
                        String::from("Generic PLA")
                    } else {
                        format!("Generic {}", tray.filament_type)
                    },
                    FilamentSource::System,
                    None,
                ),
            };
            slots.push(FilamentSlot {
                name,
                colour: slot_colour_hex(&tray.color),
                source,
                filament_id: tray.tray_info_idx,
                filament_type: tray.filament_type,
                is_support: false,
                ams_id: Some(tray.ams_id),
                slot_id: Some(tray.id),
                external_id: String::new(),
                spool_id: String::new(),
            });
            if let Some(path) = path {
                if slots.len() == 1 {
                    let _ = overlay_bbl_profile(&mut self.settings, &path);
                }
            }
        }
        self.filament_slots = slots;
        self.active_filament = 0;
        if let Some(first) = self.filament_slots.first().cloned() {
            let label = filament::filament_pick_label(first.source, &first.name);
            self.apply_filament_slot_preset(0, &label);
        }
        self.sync_filament_map();
        self.apply_group_mode();
        self.status = format!("synced {} AMS slot(s)", self.filament_slots.len());
    }

    fn persist_spools(&mut self) {
        if let Err(err) = save_inventory(bambu_protocol::default_config_dir(), &self.inventory) {
            self.status = format!("inventory.json: {err}");
        }
    }

    fn refresh_draft_from_selected(&mut self) {
        if let Some(i) = self.selected_spool {
            if let Some(spool) = self.inventory.spools.get(i) {
                self.draft_spool = self.inventory.flatten(spool);
                self.draft_location = spool.location.clone();
                self.draft_archived = spool.archived;
            }
        }
    }

    fn add_catalog_spool(&mut self, external_id: &str) {
        let Some(sku) = self.catalog.by_external_id(external_id).cloned() else {
            self.status = format!("catalog miss {external_id}");
            return;
        };
        let fid = self.inventory.add_catalog_sku(
            &sku.external_id,
            &sku.manufacturer,
            &sku.name,
            &sku.material,
            sku.density,
            sku.diameter,
            sku.net_weight,
            sku.spool_weight,
            &sku.color_name,
            &sku.color_hex,
            sku.extruder_temp,
            sku.bed_temp,
        );
        let sid = self.inventory.add_spool_for_filament(&fid);
        self.persist_spools();
        self.selected_spool = self.inventory.spools.iter().position(|s| s.id == sid);
        self.refresh_draft_from_selected();
        self.status = format!("added {} spool", sku.name);
    }

    fn save_draft_spool(&mut self) {
        let id = self.inventory.upsert_flat(&self.draft_spool);
        if let Some(spool) = self.inventory.spool_mut(&id) {
            spool.location = self.draft_location.clone();
            spool.archived = self.draft_archived;
        }
        self.persist_spools();
        self.selected_spool = self.inventory.spools.iter().position(|s| s.id == id);
        self.refresh_draft_from_selected();
        self.status = format!("saved {}", self.draft_spool.label());
    }

    fn delete_selected_spool(&mut self) {
        let Some(i) = self.selected_spool else {
            self.status = "select a spool first".into();
            return;
        };
        if let Some(id) = self.inventory.spools.get(i).map(|s| s.id.clone()) {
            self.inventory.delete_spool(&id);
        }
        self.selected_spool = None;
        self.draft_spool = FilamentSpool::default();
        self.draft_location.clear();
        self.draft_archived = false;
        self.persist_spools();
        self.status = "deleted spool".into();
    }

    fn apply_spool_use(&mut self) {
        let grams: f64 = self.draft_use.trim().parse().unwrap_or(0.0);
        let Some(i) = self.selected_spool else {
            self.status = "select a spool first".into();
            return;
        };
        if let Some(id) = self.inventory.spools.get(i).map(|s| s.id.clone()) {
            self.inventory.use_grams(&id, grams);
            self.persist_spools();
            self.refresh_draft_from_selected();
            self.draft_use.clear();
            self.status = format!("used {grams:.1} g");
        }
    }

    fn apply_spool_measure(&mut self) {
        let gross: f64 = self.draft_measure.trim().parse().unwrap_or(0.0);
        let Some(i) = self.selected_spool else {
            self.status = "select a spool first".into();
            return;
        };
        if let Some(id) = self.inventory.spools.get(i).map(|s| s.id.clone()) {
            self.inventory.measure_gross(&id, gross);
            self.persist_spools();
            self.refresh_draft_from_selected();
            self.draft_measure.clear();
            self.status = format!("measured {gross:.0} g gross");
        }
    }

    fn merge_pulled_spools(&mut self, pulled: Vec<FilamentSpool>) {
        for remote in pulled {
            self.inventory.upsert_flat(&remote);
        }
        self.persist_spools();
        self.refresh_draft_from_selected();
    }

    fn bind_spool_to_active_slot(&mut self) {
        if let Some(i) = self.selected_spool {
            if let Some(id) = self.inventory.spools.get(i).map(|s| s.id.clone()) {
                self.apply_inventory_spool(self.active_filament, &id);
                return;
            }
        }
        let spool = self.draft_spool.clone();
        if spool.filament_id.is_empty() && spool.series.is_empty() {
            self.status = "edit a spool first".into();
            return;
        }
        let sku = CatalogFilament {
            external_id: String::new(),
            manufacturer: spool.brand.clone(),
            name: spool.series.clone(),
            material: spool.material_type.clone(),
            density: 1.24,
            diameter: spool.diameter,
            net_weight: spool.initial_weight,
            spool_weight: spool.spool_weight,
            color_name: spool.color_name.clone(),
            color_hex: slot_colour_hex(&spool.color_code),
            extruder_temp: None,
            bed_temp: None,
        };
        self.apply_catalog_sku(self.active_filament, &sku);
        if let Some(slot) = self.filament_slots.get_mut(self.active_filament) {
            if !spool.filament_id.is_empty() {
                slot.filament_id = spool.filament_id.clone();
                self.settings.filament_id = spool.filament_id.clone();
            }
        }
        self.status = format!(
            "bound {} to slot {}",
            spool.label(),
            self.active_filament + 1
        );
    }

    fn selected_vol_mut(&mut self) -> Option<&mut bambu_model::ModelVolume> {
        let obj_i = self.selected_object;
        let vol_i = self.selected_volume;
        let obj = self.model_mut()?.objects.get_mut(obj_i)?;
        if obj.volumes.is_empty() {
            obj.volumes = obj.volumes_or_mesh();
        }
        obj.volumes.get_mut(vol_i)
    }

    fn sync_scene_mesh(&mut self) {
        self.needs_display_pack = true;
        self.refresh_paint_overlay();
        self.sync_gizmo();
        self.fill_xform_edits();
    }

    fn flush_display_pack(&mut self) -> Task<Message> {
        if !self.needs_display_pack || self.pack_inflight || self.load_progress.is_some() {
            return Task::none();
        }
        let Some(model) = self.model.clone() else {
            return Task::none();
        };
        self.needs_display_pack = false;
        self.pack_inflight = true;
        self.pack_gen = self.pack_gen.wrapping_add(1);
        let gen = self.pack_gen;
        let plate = self.plate;
        let bed = self.scene.bed.clone();
        let keep = self.scene.keep_solid;
        offload(
            move || {
                let meshes: Vec<bambu_geom::TriangleMesh> = model
                    .world_volumes_for_plate(plate)
                    .into_iter()
                    .map(|vol| vol.mesh)
                    .collect();
                ViewportScene::shade_meshes(meshes, bed, keep, |_, _| {})
            },
            move |shaded| Message::DisplayPacked {
                gen,
                shaded: Box::new(shaded),
            },
        )
    }

    fn model_mut(&mut self) -> Option<&mut Model> {
        self.model.as_mut().map(Arc::make_mut)
    }

    fn selected_aabb(&self) -> Option<(glam::Vec3, glam::Vec3)> {
        let model = self.model.as_deref()?;
        let inst = model
            .objects
            .get(self.selected_object)?
            .instances
            .first()
            .copied()
            .unwrap_or_default();
        let local = self.object_aabbs.get(self.selected_object).copied()?;
        let aabb = inst.transform_aabb(local);
        Some(((aabb.min + aabb.max) * 0.5, (aabb.max - aabb.min) * 0.5))
    }

    fn sync_gizmo(&mut self) {
        if !self.show_axes {
            self.scene.gizmo = None;
            return;
        }
        let dragging = self.drag_axis.is_some();
        let show = self.scene.tool.needs_gizmo() || dragging || self.gizmo_hover;
        self.scene.gizmo = if show {
            self.selected_aabb().map(|(origin, half)| AxisGizmo {
                origin,
                half,
                axis_len: screen_axis_len(self.scene.camera.distance, self.scene.viewport_height),
            })
        } else {
            None
        };
    }

    fn cursor_near_selection(&self, ndc_x: f32, ndc_y: f32, aspect: f32) -> bool {
        let Some((origin, half)) = self.selected_aabb() else {
            return false;
        };
        let pad = half.max_element().max(8.0) * 0.25 + 8.0;
        let aabb = bambu_geom::Aabb3 {
            min: origin - half - glam::Vec3::splat(pad),
            max: origin + half + glam::Vec3::splat(pad),
        };
        let (ray_o, dir) = self.scene.camera.ray_from_ndc(ndc_x, ndc_y, aspect);
        let inv = glam::Vec3::new(
            if dir.x.abs() > 1e-8 {
                1.0 / dir.x
            } else {
                f32::INFINITY
            },
            if dir.y.abs() > 1e-8 {
                1.0 / dir.y
            } else {
                f32::INFINITY
            },
            if dir.z.abs() > 1e-8 {
                1.0 / dir.z
            } else {
                f32::INFINITY
            },
        );
        aabb.intersects_ray(ray_o, inv)
    }

    fn refresh_paint_overlay(&mut self) {
        self.sync_keep_solid();
        let mut overlay = Vec::new();
        let Some(kind) = self.paint_kind else {
            self.scene.paint_overlay = overlay;
            self.sync_keep_solid();
            return;
        };
        let Some(model) = &self.model else {
            self.scene.paint_overlay = overlay;
            return;
        };
        let mut offset = 0usize;
        for obj in &model.objects {
            for vol in &obj.volumes {
                let n = vol.mesh.indices.len();
                let field = match kind {
                    PaintKind::Support => vol.triangle_support.as_slice(),
                    PaintKind::Seam => vol.triangle_seam.as_slice(),
                    PaintKind::Fuzzy => vol.triangle_fuzzy_skin.as_slice(),
                };
                for (i, paint) in field.iter().enumerate() {
                    let color = match paint {
                        TrianglePaint::Enforcer => Some(paint_overlay_color(true)),
                        TrianglePaint::Blocker => Some(paint_overlay_color(false)),
                        TrianglePaint::None => None,
                    };
                    if let Some(c) = color {
                        overlay.push((offset + i, c));
                    }
                }
                offset += n;
            }
        }
        self.scene.paint_overlay = overlay;
        self.sync_keep_solid();
    }

    fn restore_bound_printer(&mut self) -> bool {
        if self.cloud_devices.is_empty() {
            return false;
        }
        let serial = camera_serial_from_bind(&self.serial, &self.cloud_devices);
        if serial.is_empty() {
            return false;
        }
        self.apply_device(&serial);
        true
    }

    fn start_live_sync(&mut self) -> Task<Message> {
        if !self.can_monitor() {
            return Task::none();
        }
        self.live_monitor = true;
        if monitor::lan_ready(&self.host, &self.access_code) {
            self.camera_live = true;
            if self.chamber_handle.is_none() && self.camera_note.is_empty() {
                self.camera_note = "connecting JPEG :6000…".into();
            }
        } else if self.cloud_camera_ready() {
            self.camera_live = true;
            if self.chamber_handle.is_none() && self.camera_note.is_empty() {
                self.camera_note = monitor::CAMERA_CLOUD_TUTK.into();
            }
        } else if self.has_bearer && self.chamber_handle.is_none() && self.camera_note.is_empty() {
            self.camera_note = monitor::CAMERA_CLOUD_NEED_LAN.into();
        }
        self.refresh_monitor()
    }

    fn ensure_lan_access_code(&mut self) {
        if self.access_code.is_empty() && !self.serial.is_empty() {
            let serial = self.serial.clone();
            self.apply_device(&serial);
        }
    }

    fn apply_lan_from_discovered(&mut self, list: &[bambu_protocol::DiscoveredPrinter]) {
        let found = if self.serial.is_empty() {
            list.first()
        } else {
            list.iter().find(|p| p.dev_id == self.serial)
        };
        let Some(printer) = found else {
            return;
        };
        self.host = printer.dev_ip.clone();
        if self.serial.is_empty() {
            self.apply_device(&printer.dev_id);
        } else {
            let serial = self.serial.clone();
            self.apply_device(&serial);
        }
    }

    fn cloud_camera_ready(&self) -> bool {
        self.has_bearer && !self.serial.is_empty()
    }

    fn camera_job_ready(&self) -> bool {
        monitor::lan_ready(&self.host, &self.access_code) || self.cloud_camera_ready()
    }

    fn camera_job(&self) -> monitor::CameraJob {
        let session = load_cloud_session(bambu_protocol::default_config_dir()).unwrap_or_default();
        monitor::CameraJob {
            host: self.host.clone(),
            code: self.access_code.clone(),
            serial: self.serial.clone(),
            region: if self.cloud_region.is_empty() {
                session.region
            } else {
                self.cloud_region.clone()
            },
            token: session.access_token.clone(),
            refresh: session.refresh_token,
            user_id: {
                let id = if self.cloud_user.is_empty() {
                    session.user_id
                } else {
                    self.cloud_user.clone()
                };
                let id = bambu_protocol::iot_user_id(&id);
                if id.is_empty() {
                    bambu_protocol::iot_user_id(
                        &bambu_protocol::jwt_user_id(&session.access_token).unwrap_or_default(),
                    )
                } else {
                    id
                }
            },
            firmware: self.machine.ota_version.clone(),
        }
    }

    fn start_jpeg_camera(&mut self) {
        self.camera_live = true;
        if self.chamber_handle.is_none() {
            self.camera_note = if monitor::lan_ready(&self.host, &self.access_code) {
                "connecting camera (JPEG :6000 / RTSPS :322)…".into()
            } else {
                monitor::CAMERA_CLOUD_TUTK.into()
            };
        }
        self.status = "camera play".into();
    }

    fn play_camera(&mut self) -> Task<Message> {
        self.ensure_lan_access_code();
        if monitor::lan_ready(&self.host, &self.access_code) || self.cloud_camera_ready() {
            self.start_jpeg_camera();
            return Task::none();
        }
        if self.has_bearer || !self.serial.is_empty() || !self.access_code.is_empty() {
            self.camera_note = monitor::CAMERA_CLOUD_DISCOVER.into();
            self.status = self.camera_note.clone();
            return Task::perform(discover_lan_job(), Message::CameraLan);
        }
        self.camera_note = monitor::CAMERA_CLOUD_NEED_LAN.into();
        self.status = self.camera_note.clone();
        Task::none()
    }

    fn on_camera_lan(
        &mut self,
        result: Result<Vec<bambu_protocol::DiscoveredPrinter>, String>,
    ) -> Task<Message> {
        match result {
            Ok(list) => {
                self.apply_lan_from_discovered(&list);
                if self.camera_job_ready() {
                    self.start_jpeg_camera();
                } else {
                    self.camera_live = false;
                    self.camera_note = monitor::CAMERA_CLOUD_NEED_LAN.into();
                    self.status = self.camera_note.clone();
                }
            }
            Err(err) => {
                self.camera_live = false;
                self.camera_note = format!("camera: {err}");
                self.status = self.camera_note.clone();
            }
        }
        Task::none()
    }

    fn continue_slice_all(&mut self) -> Task<Message> {
        let Some(i) = self.slice_all_next else {
            return Task::none();
        };
        let next = i + 1;
        if next < self.plate_count() {
            self.slice_all_next = Some(next);
            self.set_plate(next);
            return self.slice_current();
        }
        self.slice_all_next = None;
        self.slice_all = false;
        Task::none()
    }

    pub(crate) fn ams_filament_temps(&self, ams_id: u8, slot_id: u8) -> (u16, u16) {
        let tray = self
            .ams
            .trays
            .iter()
            .find(|t| t.ams_id == ams_id && t.id == slot_id)
            .or_else(|| self.ams.vt_tray.as_ref().filter(|t| t.ams_id == ams_id));
        let new = tray
            .and_then(|t| t.temp)
            .map(|t| t.round().clamp(0.0, 300.0) as u16)
            .filter(|&t| t > 0)
            .unwrap_or(self.settings.temperature_c);
        let old = if self.machine.nozzle_temp_c > 0.0 {
            self.machine.nozzle_temp_c.round().clamp(0.0, 300.0) as u16
        } else {
            self.settings.temperature_c
        };
        (old, new)
    }

    fn refresh_monitor(&mut self) -> Task<Message> {
        if self.monitor_fetching {
            return Task::none();
        }
        self.monitor_fetching = true;
        let host = self.host.clone();
        let code = self.access_code.clone();
        let serial = self.serial.clone();
        let send_via = self.send_via;
        Task::perform(
            async move {
                monitor::fetch_monitor(send_via, host, code, serial)
                    .await
                    .map(Box::new)
            },
            Message::Status,
        )
    }

    fn hms_cmd(&self, resume: bool) -> Option<PrintCmd> {
        let hms = self.machine.hms.first()?;
        let err = hms.long_error_code();
        let job = self.machine.job_id.clone();
        Some(if resume {
            PrintCmd::HmsResume { err, job }
        } else {
            PrintCmd::HmsIgnore { err, job }
        })
    }

    fn run_print_cmd(&self, cmd: PrintCmd) -> Task<Message> {
        let host = self.host.clone();
        let code = self.access_code.clone();
        let serial = self.serial.clone();
        let send_via = self.send_via;
        Task::perform(
            async move { monitor::run_cmd(send_via, host, code, serial, cmd).await },
            Message::PrintControl,
        )
    }
}

#[derive(Debug, Clone, Copy)]
enum PaintKind {
    Support,
    Seam,
    Fuzzy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RackPending {
    Move(u8),
    ReadAll,
}

/// 1×1 RGB PNG used as a 3MF plate thumbnail in tests.
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

#[derive(Debug, Clone)]
enum PrintCmd {
    Pause,
    Resume,
    Stop,
    Speed(u8),
    Light(bool),
    Bed(u16),
    Nozzle(u16),
    Fan {
        index: u8,
        speed: u8,
    },
    AmsLoad {
        ams_id: u8,
        slot_id: u8,
        old_temp: u16,
        new_temp: u16,
    },
    AmsUnload {
        ams_id: u8,
    },
    AmsDry {
        ams_id: u8,
        filament: String,
        temp: u16,
        hours: u16,
        rotate: bool,
    },
    AmsDryStop {
        ams_id: u8,
    },
    RackMove(u8),
    RackRead(u32),
    RackConfirm(u32),
    HmsResume {
        err: String,
        job: String,
    },
    HmsIgnore {
        err: String,
        job: String,
    },
}

fn parse_temp_c(raw: &str) -> u16 {
    raw.trim()
        .parse::<f32>()
        .map(|n| n.round().clamp(0.0, 300.0) as u16)
        .unwrap_or(0)
}

pub(crate) fn slot_colour_hex(raw: &str) -> String {
    let n = bambu_config::normalize_filament_colour(raw);
    if n.is_empty() {
        String::from("#FFFFFFFF")
    } else {
        n
    }
}

fn default_filament_pick(system: &[BblProfileEntry]) -> (String, FilamentSource) {
    if let Some(paths) = bambu_config::bbl_oracle_paths() {
        let stem = paths
            .filament
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Generic PLA");
        if system.iter().any(|p| p.name == stem) {
            return (stem.to_string(), FilamentSource::System);
        }
    }
    let name = system
        .iter()
        .find(|p| p.name == "Generic PLA")
        .or_else(|| system.first())
        .map(|p| p.name.clone())
        .unwrap_or_else(|| String::from("Generic PLA"));
    (name, FilamentSource::System)
}

async fn discover_lan_job() -> Result<Vec<bambu_protocol::DiscoveredPrinter>, String> {
    std::thread::spawn(|| {
        bambu_protocol::discover(std::time::Duration::from_secs(3)).map_err(|err| err.to_string())
    })
    .join()
    .unwrap_or_else(|_| Err("discover thread panicked".into()))
}

fn pull_cloud_filaments() -> Result<Vec<FilamentSpool>, String> {
    let dir = bambu_protocol::default_config_dir();
    let session = load_cloud_session(&dir).map_err(|err| err.to_string())?;
    if !session.has_bearer() {
        return Err("cloud_token missing".into());
    }
    let api = CloudApi::new(
        &session.region,
        &session.access_token,
        &session.refresh_token,
    );
    api.list_filaments().map_err(|err| err.to_string())
}

fn push_cloud_filaments(spools: &[FilamentSpool]) -> Result<String, String> {
    let dir = bambu_protocol::default_config_dir();
    let session = load_cloud_session(&dir).map_err(|err| err.to_string())?;
    if !session.has_bearer() {
        return Err("cloud_token missing".into());
    }
    let api = CloudApi::new(
        &session.region,
        &session.access_token,
        &session.refresh_token,
    );
    let mut pushed = 0usize;
    for spool in spools {
        if spool.spool_id.is_empty() {
            api.create_filament(spool).map_err(|err| err.to_string())?;
        } else {
            api.update_filament(spool).map_err(|err| err.to_string())?;
        }
        pushed += 1;
    }
    Ok(format!("pushed {pushed} spool(s)"))
}

async fn pick_mesh_path() -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .add_filter("Projects", &["3mf", "3MF"])
        .add_filter("Meshes", &["3mf", "3MF", "stl", "STL"])
        .add_filter("3MF", &["3mf", "3MF"])
        .add_filter("STL", &["stl", "STL"])
        .pick_file()
        .await
        .map(|file| file.path().to_path_buf())
}

async fn pick_import_path() -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .add_filter(
            "Models",
            &[
                "3mf", "3MF", "stl", "STL", "obj", "OBJ", "step", "stp", "STEP", "svg", "SVG",
            ],
        )
        .add_filter("3MF", &["3mf", "3MF"])
        .add_filter("STL", &["stl", "STL"])
        .add_filter("OBJ", &["obj", "OBJ"])
        .add_filter("STEP", &["step", "stp", "STEP"])
        .add_filter("SVG", &["svg", "SVG"])
        .pick_file()
        .await
        .map(|file| file.path().to_path_buf())
}

async fn pick_save_project_path(name: String) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .add_filter("3MF project", &["3mf", "3MF"])
        .set_file_name(&name)
        .save_file()
        .await
        .map(|file| file.path().to_path_buf())
}

async fn pick_export_path() -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .add_filter("G-code 3MF", &["gcode.3mf"])
        .set_file_name("plate.gcode.3mf")
        .save_file()
        .await
        .map(|file| file.path().to_path_buf())
}

pub(crate) fn write_exported_3mf(gcode: &str, path: &std::path::Path) -> Result<(), String> {
    let bytes = bambu_protocol::pack_gcode_3mf(gcode).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

fn tree_chip<'a>(label: String, active: bool, message: Message) -> Element<'a, Message> {
    let color = if active {
        iced::Color::WHITE
    } else {
        theme::TEXT_MUTED
    };
    container(
        button(text(label).size(12).color(color))
            .padding([3, 8])
            .width(Fill)
            .style(move |_, status| theme::process_tab(active, status))
            .on_press(message),
    )
    .width(Fill)
    .style(move |_| {
        if active {
            container::Style {
                background: Some(iced::Background::Color(theme::PREPARE)),
                border: iced::Border {
                    radius: theme::RADIUS.into(),
                    width: 0.0,
                    color: iced::Color::TRANSPARENT,
                },
                ..container::Style::default()
            }
        } else {
            container::Style::default()
        }
    })
    .into()
}

fn offload<T, M>(
    f: impl FnOnce() -> T + Send + 'static,
    map: impl FnOnce(T) -> M + Send + 'static,
) -> Task<M>
where
    T: Send + 'static,
    M: Send + 'static,
{
    Task::perform(
        async move { tokio::task::spawn_blocking(f).await.expect("worker") },
        map,
    )
}

fn load_model_stream(job: LoadJob) -> impl iced::futures::Stream<Item = Message> {
    iced::stream::channel(32, async move |output| {
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
        let _ = std::thread::Builder::new()
            .name("bambu-load".into())
            .spawn(move || {
                load_model_worker(job, output);
                let _ = done_tx.send(());
            });
        let _ = tokio::task::spawn_blocking(move || {
            let _ = done_rx.recv();
        })
        .await;
    })
}

fn emit_latest(tx: &mut iced::futures::channel::mpsc::Sender<Message>, msg: Message) {
    match tx.try_send(msg) {
        Ok(()) => {}
        Err(err) if err.is_full() => {}
        Err(_) => {}
    }
}

fn emit_block(tx: &mut iced::futures::channel::mpsc::Sender<Message>, mut msg: Message) {
    loop {
        match tx.try_send(msg) {
            Ok(()) => return,
            Err(err) if err.is_full() => {
                msg = err.into_inner();
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(_) => return,
        }
    }
}

fn emit_stage(tx: &mut iced::futures::channel::mpsc::Sender<Message>, gen: u64, stage: LoadStage) {
    emit_latest(
        tx,
        Message::LoadProgress {
            gen,
            label: stage.label(),
            fraction: stage.fraction(),
        },
    );
}

fn load_model_worker(job: LoadJob, mut tx: iced::futures::channel::mpsc::Sender<Message>) {
    let gen = job.gen;
    let result = (|| {
        let t0 = std::time::Instant::now();
        let label = job
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("mesh")
            .to_string();
        let mut model = load_model_with_progress(&job.path, |stage| {
            emit_stage(&mut tx, gen, stage);
        })
        .or_else(|err| {
            if is_unsupported_import(&job.path) {
                Err(err.to_string())
            } else {
                let mesh = load_mesh(&job.path).map_err(|e| e.to_string())?;
                Ok(Model::from_mesh(&label, mesh))
            }
        })?;
        let mut bed = job.bed;
        if job.apply_settings {
            if let Some(settings) = &model.settings {
                bed = settings.bed_shape();
            }
        }
        emit_latest(
            &mut tx,
            Message::LoadProgress {
                gen,
                label: "Placing on build plate…".into(),
                fraction: 0.59,
            },
        );
        model.place_on_bed_if_needed(&bed);
        let object_aabbs: Vec<bambu_geom::Aabb3> =
            model.objects.iter().filter_map(|o| o.mesh.aabb()).collect();
        let meshes: Vec<bambu_geom::TriangleMesh> = model
            .world_volumes_for_plate(0)
            .into_iter()
            .map(|vol| vol.mesh)
            .collect();
        let shaded = ViewportScene::shade_meshes(meshes, bed, true, |label, fraction| {
            emit_latest(
                &mut tx,
                Message::LoadProgress {
                    gen,
                    label: label.into(),
                    fraction,
                },
            );
        });
        Ok(Box::new(LoadedModel {
            label,
            path: job.path,
            model,
            apply_settings: job.apply_settings,
            load_ms: t0.elapsed().as_millis(),
            shaded,
            object_aabbs,
        }))
    })();
    emit_block(&mut tx, Message::ModelLoaded(result));
}

fn file_mtime(path: &std::path::Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn png_to_handle(bytes: &[u8]) -> Option<iced::widget::image::Handle> {
    let (w, h, rgba) = crate::decode_png(bytes).ok()?;
    Some(iced::widget::image::Handle::from_rgba(w, h, rgba))
}

fn is_project_3mf(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("3mf"))
        .unwrap_or(false)
        && !path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.to_ascii_lowercase().ends_with(".gcode.3mf"))
}

fn is_unsupported_import(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "obj" | "svg" | "step" | "stp"
    )
}

fn on_window_event(
    event: iced::Event,
    status: iced::event::Status,
    _id: iced::window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            if status == iced::event::Status::Captured {
                return None;
            }
            if !(modifiers.command() || modifiers.control()) {
                return None;
            }
            match key.as_ref() {
                iced::keyboard::Key::Character("n" | "N") => Some(Message::NewProject),
                iced::keyboard::Key::Character("o" | "O") => Some(Message::OpenModel),
                iced::keyboard::Key::Character("s" | "S") if modifiers.shift() => {
                    Some(Message::SaveProjectAs)
                }
                iced::keyboard::Key::Character("s" | "S") => Some(Message::SaveProject),
                iced::keyboard::Key::Character("i" | "I") => Some(Message::ImportModel),
                _ => None,
            }
        }
        _ => None,
    }
}

fn run_slice_job(job: SliceJob) -> Result<Box<SliceOutcome>, String> {
    let SliceJob {
        settings,
        mesh,
        volumes,
        objects,
    } = job;
    if let Some(objects) = objects {
        let mut results = Vec::new();
        for vols in objects {
            let (result, _) =
                slice_volumes_with_gpu_or_cpu(&vols, &settings).map_err(|err| err.to_string())?;
            results.push(result);
        }
        check_print_path_conflicts(&results).map_err(|err| err.to_string())?;
        return Ok(Box::new(SliceOutcome::Objects { results, settings }));
    }
    if let Some(vols) = volumes {
        let (result, backend) =
            slice_volumes_with_gpu_or_cpu(&vols, &settings).map_err(|err| err.to_string())?;
        return Ok(Box::new(SliceOutcome::Single {
            result,
            backend: backend.to_string(),
            settings,
        }));
    }
    let (result, backend) =
        slice_with_gpu_or_cpu(&mesh, &settings).map_err(|err| err.to_string())?;
    Ok(Box::new(SliceOutcome::Single {
        result,
        backend: backend.to_string(),
        settings,
    }))
}

pub(crate) fn role_legend() -> String {
    format!(
        "{} · {} · {} · {} · {}",
        ExtrusionRole::OuterWall.label(),
        ExtrusionRole::InnerWall.label(),
        ExtrusionRole::Infill.label(),
        ExtrusionRole::Support.label(),
        ExtrusionRole::PrimeTower.label()
    )
}

pub(crate) fn format_eta(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m {sec}s")
    }
}

fn calibration_block_path() -> Option<PathBuf> {
    let rel = PathBuf::from("tests/calibration_block/3D+Printer+Test.3mf");
    let mut candidates = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(&rel)];
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(&rel));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn seed_tray(ams_id: u8, id: u8, filament_type: &str, color: &str, remain: Option<u8>) -> AmsTray {
    AmsTray {
        id,
        ams_id,
        filament_type: filament_type.into(),
        color: color.into(),
        remain,
        ..AmsTray::default()
    }
}

fn lan_from(host: String, code: String, serial: String) -> LanBackend {
    let creds = bambu_protocol::load_from_dir(bambu_protocol::default_config_dir())
        .unwrap_or_else(|_| Default::default());
    LanBackend::new(host, code)
        .with_serial(serial)
        .with_credentials(creds)
}

fn default_slice_settings() -> SliceSettings {
    match bambu_config::bbl_oracle_paths() {
        Some(paths) => {
            let mut settings =
                load_bbl_process(&paths.process).unwrap_or_else(|_| SliceSettings::bbl_0_20());
            let _ = overlay_bbl_profile(&mut settings, &paths.machine);
            let _ = overlay_bbl_profile(&mut settings, &paths.filament);
            settings
        }
        None => SliceSettings::bbl_0_20(),
    }
}

#[cfg(test)]
mod device_sync {
    use super::*;
    use bambu_protocol::DiscoveredPrinter;

    #[test]
    fn discover_fills_lan_code_and_starts_live_monitor() {
        let mut app = App::new_for_gui_test();
        app.imported_printers.push(StudioPrinter {
            serial: "01P00A000000001".into(),
            access_code: "12345678".into(),
        });
        let _ = app.update(Message::Discovered(Ok(vec![DiscoveredPrinter {
            dev_id: "01P00A000000001".into(),
            dev_ip: "192.168.1.20".into(),
            dev_name: "P1S".into(),
            ..Default::default()
        }])));
        assert_eq!(app.host, "192.168.1.20");
        assert_eq!(app.serial, "01P00A000000001");
        assert_eq!(app.access_code, "12345678");
        assert!(app.live_monitor);
        assert!(app.camera_live);
    }

    #[test]
    fn device_tab_starts_live_when_lan_ready() {
        let mut app = App::new_for_gui_test();
        app.host = "192.168.1.20".into();
        app.access_code = "12345678".into();
        let _ = app.update(Message::Workspace(Workspace::Device));
        assert!(app.live_monitor);
        assert!(app.camera_live);
        assert_eq!(app.workspace, Workspace::Device);
        assert!(app.camera_note.contains("JPEG"));
    }

    #[test]
    fn seeded_device_ams_shows_two_unit_readings() {
        let mut app = App::new_for_gui_test();
        app.seed_device_monitor();
        assert_eq!(app.ams.units.len(), 2);
        let a = monitor::ams_unit_summary(&app.ams.units[0]);
        let b = monitor::ams_unit_summary(&app.ams.units[1]);
        assert!(a.contains("RH2"));
        assert!(b.contains("RH4"));
        assert_ne!(a, b);
        let snap = app.snapshot();
        assert!(snap.labels.iter().any(|s| s == "File"));
    }

    #[test]
    fn status_keeps_ams_and_rack_when_push_omits_them() {
        let mut app = App::new_for_gui_test();
        app.ams.trays.push(AmsTray {
            id: 0,
            filament_type: "PLA".into(),
            ..Default::default()
        });
        app.machine.nozzle_rack.supported = true;
        app.machine.ota_version = "01.02.00.00".into();
        app.monitor_fetching = true;
        let snap = MonitorSnapshot {
            line: "IDLE".into(),
            machine: MachineState::default(),
            ams: AmsState::default(),
            hms_lines: Vec::new(),
        };
        let _ = app.update(Message::Status(Ok(Box::new(snap))));
        assert_eq!(app.ams.trays.len(), 1);
        assert_eq!(app.ams.trays[0].filament_type, "PLA");
        assert!(app.machine.nozzle_rack.supported);
        assert_eq!(app.machine.ota_version, "01.02.00.00");
        assert!(!app.monitor_fetching);
    }

    #[test]
    fn status_keeps_rack_slots_when_holder_empty() {
        let mut app = App::new_for_gui_test();
        app.machine.nozzle_rack.supported = true;
        app.machine.nozzle_rack.status = 0;
        app.machine.nozzle_rack.position = 1;
        app.machine
            .nozzle_rack
            .toolhead
            .push(bambu_device::NozzleSlot {
                id: 0,
                diameter: 0.4,
                ..Default::default()
            });
        app.machine.nozzle_rack.rack.push(bambu_device::NozzleSlot {
            id: 0,
            diameter: 0.2,
            ..Default::default()
        });
        assert_eq!(app.machine.nozzle_rack.toolhead.len(), 1);
        app.monitor_fetching = true;
        let mut machine = MachineState::default();
        machine.nozzle_rack.supported = true;
        machine.nozzle_rack.status = -1;
        machine.nozzle_rack.position = -1;
        let snap = MonitorSnapshot {
            line: "IDLE".into(),
            machine,
            ams: AmsState::default(),
            hms_lines: Vec::new(),
        };
        let _ = app.update(Message::Status(Ok(Box::new(snap))));
        assert_eq!(app.machine.nozzle_rack.toolhead.len(), 1);
        assert_eq!(app.machine.nozzle_rack.rack.len(), 1);
        assert_eq!(app.machine.nozzle_rack.status, 0);
        assert_eq!(app.machine.nozzle_rack.position, 1);
        assert!(app.machine.nozzle_rack.supported);
        assert!(!app.monitor_fetching);
    }

    #[test]
    fn hotend_rack_hidden_until_holder_reported() {
        let mut app = App::new_for_gui_test();
        app.seed_device_monitor();
        assert!(!app.machine.nozzle_rack.supported);
        assert!(app.rack_pane().is_none());
        app.machine.nozzle_rack.supported = true;
        assert!(app.rack_pane().is_some());
        let _ = app.update(Message::RackReadAll);
        assert_eq!(app.rack_pending, Some(RackPending::ReadAll));
        let _ = app.update(Message::RackMove(1));
        assert_eq!(app.rack_pending, Some(RackPending::Move(1)));
    }

    #[test]
    fn ams_dryness_defaults_to_no_spool_rotate() {
        let app = App::new_for_gui_test();
        assert!(!app.dry_rotate);
    }

    #[test]
    fn new_project_clears_plate() {
        let mut app = App::new_for_gui_test();
        assert!(!app.model.as_ref().unwrap().objects.is_empty());
        let _ = app.update(Message::NewProject);
        assert!(app.model.as_ref().unwrap().objects.is_empty());
        assert!(app.project_file.is_none());
        assert_eq!(app.workspace, Workspace::Prepare);
        assert_eq!(app.status, "new project");
    }

    #[test]
    fn open_recent_switches_to_prepare_with_progress() {
        let mut app = App::new_for_gui_test();
        app.workspace = Workspace::Home;
        let path = std::env::temp_dir().join("bambu-ui-open-progress.3mf");
        let _ = std::fs::remove_file(&path);
        App::write_recent_preview_3mf(&path).unwrap();
        let _ = app.update(Message::OpenRecent(path.clone()));
        assert_eq!(app.workspace, Workspace::Prepare);
        assert!(app.busy);
        let progress = app.load_progress.as_ref().expect("progress overlay");
        assert!(progress.fraction > 0.0);
        assert!(progress.fraction < 1.0);
        assert!(progress.label.to_ascii_lowercase().contains("open"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_progress_message_updates_overlay() {
        let mut app = App::new_for_gui_test();
        app.workspace = Workspace::Home;
        app.busy = true;
        app.load_gen = 3;
        let _ = app.update(Message::LoadProgress {
            gen: 3,
            label: "Parsing object meshes (0/1)…".into(),
            fraction: 0.2,
        });
        assert_eq!(app.workspace, Workspace::Home);
        let progress = app.load_progress.as_ref().expect("progress");
        assert_eq!(progress.label, "Parsing object meshes (0/1)…");
        assert!((progress.fraction - 0.2).abs() < f32::EPSILON);
        assert_eq!(app.status, "Parsing object meshes (0/1)…");
        let _ = app.update(Message::LoadProgress {
            gen: 99,
            label: "stale".into(),
            fraction: 0.9,
        });
        assert_eq!(
            app.load_progress.as_ref().unwrap().label,
            "Parsing object meshes (0/1)…"
        );
    }

    #[test]
    fn chamber_jpeg_stores_image_handle() {
        let mut app = App::new_for_gui_test();
        let rgba = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let _ = app.update(Message::ChamberShot(Ok(ChamberResult::Jpeg {
            bytes: 128,
            width: 2,
            height: 2,
            rgba,
        })));
        assert!(app.chamber_handle.is_some());
        assert_eq!((app.chamber_width, app.chamber_height), (2, 2));
        assert!(app.camera_note.is_empty());
    }

    #[test]
    fn chamber_shot_two_frames_replace_handle_size() {
        let mut app = App::new_for_gui_test();
        let _ = app.update(Message::ChamberShot(Ok(ChamberResult::Jpeg {
            bytes: 10,
            width: 8,
            height: 8,
            rgba: vec![255; 8 * 8 * 4],
        })));
        assert_eq!((app.chamber_width, app.chamber_height), (8, 8));
        let _ = app.update(Message::ChamberShot(Ok(ChamberResult::Jpeg {
            bytes: 20,
            width: 16,
            height: 8,
            rgba: vec![0; 16 * 8 * 4],
        })));
        assert!(app.chamber_handle.is_some());
        assert_eq!((app.chamber_width, app.chamber_height), (16, 8));
    }

    #[test]
    fn camera_play_stop_toggle_live_flag() {
        let mut app = App::new_for_gui_test();
        app.host = "192.168.1.20".into();
        app.access_code = "12345678".into();
        let _ = app.update(Message::CameraPlay);
        assert!(app.camera_live);
        let _ = app.update(Message::CameraStop);
        assert!(!app.camera_live);
    }

    #[test]
    fn camera_play_cloud_without_lan_explains_jpeg() {
        let mut app = App::new_for_gui_test();
        app.has_bearer = true;
        let _ = app.update(Message::CameraPlay);
        assert!(!app.camera_live);
        assert!(app.camera_note.contains("JPEG"));
        let _ = app.update(Message::CameraLan(Ok(vec![])));
        assert!(!app.camera_live);
        assert_eq!(app.camera_note, monitor::CAMERA_CLOUD_NEED_LAN);
    }

    #[test]
    fn pick_device_uses_bind_access_code() {
        let mut app = App::new_for_gui_test();
        app.cloud_devices.push(CloudDevice {
            dev_id: "01H2C0000000001".into(),
            name: "H2C".into(),
            online: true,
            dev_name: "H2C".into(),
            access_code: "abcd1234".into(),
            dev_ver: String::new(),
        });
        let label = app.cloud_devices[0].label();
        let _ = app.update(Message::PickDevice(label));
        assert_eq!(app.serial, "01H2C0000000001");
        assert_eq!(app.access_code, "abcd1234");
    }

    #[test]
    fn camera_play_cloud_uses_ssdp_ip_and_imported_code() {
        let mut app = App::new_for_gui_test();
        app.has_bearer = true;
        app.serial = "01P00A000000001".into();
        app.imported_printers.push(StudioPrinter {
            serial: "01P00A000000001".into(),
            access_code: "12345678".into(),
        });
        let _ = app.update(Message::CameraLan(Ok(vec![DiscoveredPrinter {
            dev_id: "01P00A000000001".into(),
            dev_ip: "192.168.1.20".into(),
            dev_name: "P1S".into(),
            ..Default::default()
        }])));
        assert_eq!(app.host, "192.168.1.20");
        assert_eq!(app.access_code, "12345678");
        assert!(app.camera_live);
    }

    #[test]
    fn camera_play_ignores_ssdp_for_other_serial() {
        let mut app = App::new_for_gui_test();
        app.has_bearer = true;
        app.serial = "01P00A000000001".into();
        app.imported_printers.push(StudioPrinter {
            serial: "01P00A000000001".into(),
            access_code: "12345678".into(),
        });
        let _ = app.update(Message::CameraLan(Ok(vec![DiscoveredPrinter {
            dev_id: "01X00A999999999".into(),
            dev_ip: "10.0.0.9".into(),
            ..Default::default()
        }])));
        assert!(app.host.is_empty());
        assert!(app.camera_live);
        assert_eq!(app.camera_note, monitor::CAMERA_CLOUD_TUTK);
    }

    #[test]
    fn camera_play_cloud_serial_starts_tutk_without_lan() {
        let mut app = App::new_for_gui_test();
        app.has_bearer = true;
        app.serial = "01P00A000000001".into();
        let _ = app.update(Message::CameraPlay);
        assert!(app.camera_live);
        assert_eq!(app.camera_note, monitor::CAMERA_CLOUD_TUTK);
    }

    #[test]
    fn rtsps_note_does_not_stop_jpeg_retry() {
        let mut app = App::new_for_gui_test();
        app.camera_live = true;
        let _ = app.update(Message::ChamberShot(Ok(ChamberResult::Rtsps {
            detail: "RTSPS :322 (no H.264 this pass) rtsps://x · v=0".into(),
        })));
        assert!(app.camera_live);
        assert!(app.camera_note.contains("RTSPS :322"));
    }

    #[test]
    fn export_writes_gcode_3mf_without_lan_send() {
        let mut app = App::new_for_gui_test();
        app.last_gcode = Some("; LAYER\nG1 X1\n".into());
        app.print_export = true;
        app.host.clear();
        app.access_code.clear();
        let _ = app.update(Message::Send);
        assert!(!app.status.contains("printer IP"));
        assert!(!app.status.contains("FTPS"));

        let path = std::env::temp_dir().join("bambu-ui-export-test.gcode.3mf");
        let _ = std::fs::remove_file(&path);
        write_exported_3mf(app.last_gcode.as_deref().unwrap(), &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"PK"));
        let _ = app.update(Message::ExportPicked(Some(path.clone())));
        assert!(app.status.contains("exported"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn send_without_gcode_stays_slice_before_send() {
        let mut app = App::new_for_gui_test();
        app.print_export = false;
        app.last_gcode = None;
        let _ = app.update(Message::Send);
        assert_eq!(app.status, "slice before send");
    }

    #[test]
    fn slice_all_visits_each_plate() {
        let mut app = App::new_for_gui_test();
        if let Some(model) = app.model.as_mut().map(Arc::make_mut) {
            model.plates.push(bambu_model::PartPlate {
                name: "Plate 2".into(),
                object_indices: vec![0],
                locked: false,
            });
        }
        assert_eq!(app.plate_count(), 2);
        let _ = app.update(Message::SliceAll(true));
        let _ = app.update(Message::Slice);
        assert_eq!(app.plate, 0);
        assert_eq!(app.slice_all_next, Some(0));
        assert!(app.busy);
        app.busy = false;
        let _ = app.continue_slice_all();
        assert_eq!(app.plate, 1);
        assert_eq!(app.slice_all_next, Some(1));
        assert!(app.busy);
        app.busy = false;
        let _ = app.continue_slice_all();
        assert!(app.slice_all_next.is_none());
        assert!(!app.slice_all);
    }

    #[test]
    fn preview_without_gcode_starts_slice() {
        let mut app = App::new_for_gui_test();
        assert!(app.last_gcode.is_none());
        let _ = app.update(Message::Workspace(Workspace::Preview));
        assert!(app.busy);
        assert_eq!(app.status, "slicing…");
    }

    #[test]
    fn preview_with_gcode_does_not_reslice() {
        let mut app = App::new_for_gui_test();
        app.last_gcode = Some("; already".into());
        let _ = app.update(Message::Workspace(Workspace::Preview));
        assert!(!app.busy);
        assert_ne!(app.status, "slicing…");
    }

    #[test]
    fn stop_without_confirm_does_not_send() {
        let mut app = App::new_for_gui_test();
        let _ = app.update(Message::Stop);
        assert!(app.stop_confirm);
        let _ = app.update(Message::StopCancel);
        assert!(!app.stop_confirm);
    }

    #[test]
    fn print_speed_ignored_when_idle() {
        let mut app = App::new_for_gui_test();
        app.machine.gcode_state = "IDLE".into();
        app.toasts.clear();
        let _ = app.update(Message::PrintSpeed(2));
        assert!(app.toasts.iter().any(|t| t.contains("during printing")));
        app.machine.gcode_state = "RUNNING".into();
        app.toasts.clear();
        let _ = app.update(Message::PrintSpeed(2));
        assert!(!app.toasts.iter().any(|t| t.contains("during printing")));
    }

    #[test]
    fn ams_load_uses_tray_and_nozzle_temps() {
        let mut app = App::new_for_gui_test();
        app.settings.temperature_c = 210;
        app.machine.nozzle_temp_c = 219.0;
        app.ams.trays = vec![AmsTray {
            id: 1,
            ams_id: 0,
            filament_type: "PLA".into(),
            temp: Some(255.0),
            ..AmsTray::default()
        }];
        assert_eq!(app.ams_filament_temps(0, 1), (219, 255));
        app.ams.trays[0].temp = None;
        assert_eq!(app.ams_filament_temps(0, 1), (219, 210));
        app.machine.nozzle_temp_c = 0.0;
        assert_eq!(app.ams_filament_temps(0, 1), (210, 210));
    }

    #[test]
    fn support_paint_overlay_hidden_until_tool() {
        let mut app = App::new_for_gui_test();
        app.seed_support_paint_cube();
        assert_eq!(
            app.paint_overlay_count(),
            0,
            "default Prepare is plastic only"
        );
        let _ = app.update(Message::PaintSupport);
        assert!(
            app.paint_overlay_count() >= 2,
            "support paint tool should show imported blockers"
        );
        let _ = app.update(Message::PaintSeam);
        assert_eq!(
            app.paint_overlay_count(),
            0,
            "seam tool must not show support blockers"
        );
        let _ = app.update(Message::PaintClear);
        assert_eq!(app.paint_overlay_count(), 0);
    }

    #[test]
    fn axes_toggle_and_transform_tool_show_gizmo() {
        let mut app = App::new_for_gui_test();
        assert!(app.show_axes());
        assert!(!app.gizmo_visible(), "Orbit hides axes until hover");
        let _ = app.update(Message::PlaterTool(PlaterTool::Move));
        assert!(
            app.gizmo_visible(),
            "Move always shows axes when toggle is on"
        );
        let _ = app.update(Message::ShowAxes(false));
        assert!(!app.show_axes());
        assert!(!app.gizmo_visible());
        let _ = app.update(Message::ShowAxes(true));
        assert!(app.gizmo_visible());
        let _ = app.update(Message::PlaterTool(PlaterTool::Orbit));
        assert!(!app.gizmo_visible());
        let _ = app.update(Message::Viewport(ViewportEvent::CursorMoved {
            ndc_x: 0.0,
            ndc_y: 0.0,
            aspect: 1.5,
            viewport_h: 800.0,
        }));
        // NDC origin may or may not hit the cube depending on camera; hover uses AABB ray.
        let _ = app.update(Message::Viewport(ViewportEvent::CursorLeft));
        assert!(!app.gizmo_visible());
    }
}
