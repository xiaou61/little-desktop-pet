use std::{
    ffi::c_void,
    fmt,
    fs::OpenOptions,
    io::{self, Write},
    mem::size_of,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr::{copy_nonoverlapping, null_mut},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
};

use image::{RgbaImage, imageops::FilterType};
use serde::{Deserialize, Serialize};
use windows::{
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::Gdi::{
            AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
            CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject,
            EnumDisplayMonitors, GetDC, GetMonitorInfoW, HBITMAP, HDC, HGDIOBJ, HMONITOR,
            MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, ReleaseDC, ScreenToClient,
            SelectObject,
        },
        Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW},
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::{
            HiDpi::GetDpiForSystem,
            Input::KeyboardAndMouse::{ReleaseCapture, SetCapture},
            WindowsAndMessaging::{
                CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
                GWLP_USERDATA, GetCursorPos, GetMessageW, GetSystemMetrics, GetWindowLongPtrW,
                GetWindowRect, HTCLIENT, HTTRANSPARENT, HWND_TOPMOST, IDC_ARROW, IsWindow,
                KillTimer, LoadCursorW, MA_NOACTIVATE, MSG, PostMessageW, PostQuitMessage,
                RegisterClassExW, SM_CXDRAG, SM_CYDRAG, SMTO_ABORTIFHUNG, SW_HIDE,
                SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SendMessageTimeoutW,
                SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, ULW_ALPHA,
                UnregisterClassW, UpdateLayeredWindow, WINDOW_EX_STYLE, WM_APP, WM_CAPTURECHANGED,
                WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ERASEBKGND,
                WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_NCCREATE,
                WM_NCDESTROY, WM_NCHITTEST, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
                WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
            },
        },
    },
    core::{BOOL, Error as WindowsError, PCWSTR, w},
};

use crate::{
    model::CommandError,
    panel_model::{PetAnchor, PhysicalRect as AnchorRect},
    pet_preferences::{load_size_percent, normalize_size_percent, save_size_percent},
    pet_skin_preferences::{load_skin_id, save_skin_id},
    pet_skins::{DEFAULT_SKIN_ID, SkinDefinition, decode_skin, skin_by_id},
};

const STATE_VERSION: u32 = 1;
const BASE_DPI: u32 = 96;
const WINDOW_LOGICAL_WIDTH: i32 = 220;
const WINDOW_LOGICAL_HEIGHT: i32 = 240;
const PET_LOGICAL_WIDTH: i32 = 200;
const PET_LOGICAL_HEIGHT: i32 = 220;
const DEFAULT_MARGIN_LOGICAL: i32 = 24;
const ALPHA_HIT_THRESHOLD: u8 = 32;
const TIMER_ID: usize = 1;
const TIMER_INTERVAL_MS: u32 = 250;
const FRAME_COUNT: usize = 8;
const CLASS_NAME: PCWSTR = w!("SmallDesktopPetWindow");

const MSG_SHOW: u32 = WM_APP + 1;
const MSG_HIDE: u32 = WM_APP + 2;
const MSG_ENSURE_VISIBLE: u32 = WM_APP + 3;
const MSG_SAVE_POSITION: u32 = WM_APP + 4;
const MSG_SHUTDOWN: u32 = WM_APP + 5;
const MSG_SET_SIZE: u32 = WM_APP + 6;
const MSG_PREVIEW_SIZE: u32 = WM_APP + 7;
const MSG_SET_SKIN: u32 = WM_APP + 8;
const MESSAGE_TIMEOUT_MS: u32 = 3_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PhysicalPoint {
    x: i32,
    y: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PhysicalSize {
    width: i32,
    height: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PhysicalRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl PhysicalRect {
    fn from_point_size(point: PhysicalPoint, size: PhysicalSize) -> Self {
        Self {
            left: point.x,
            top: point.y,
            right: point.x.saturating_add(size.width),
            bottom: point.y.saturating_add(size.height),
        }
    }

    fn contains(self, other: Self) -> bool {
        other.left >= self.left
            && other.top >= self.top
            && other.right <= self.right
            && other.bottom <= self.bottom
    }
}

impl From<RECT> for PhysicalRect {
    fn from(value: RECT) -> Self {
        Self {
            left: value.left,
            top: value.top,
            right: value.right,
            bottom: value.bottom,
        }
    }
}

impl From<PhysicalRect> for RECT {
    fn from(value: PhysicalRect) -> Self {
        Self {
            left: value.left,
            top: value.top,
            right: value.right,
            bottom: value.bottom,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MonitorWorkArea {
    rect: PhysicalRect,
    primary: bool,
}

fn default_bottom_right(
    work_area: PhysicalRect,
    window_size: PhysicalSize,
    margin: i32,
) -> PhysicalPoint {
    clamp_to_work_area(
        PhysicalPoint {
            x: work_area
                .right
                .saturating_sub(window_size.width)
                .saturating_sub(margin),
            y: work_area
                .bottom
                .saturating_sub(window_size.height)
                .saturating_sub(margin),
        },
        window_size,
        work_area,
    )
}

fn restore_position(
    saved: Option<PhysicalPoint>,
    monitors: &[MonitorWorkArea],
    window_size: PhysicalSize,
    margin: i32,
) -> PhysicalPoint {
    if let Some(saved) = saved {
        let window = PhysicalRect::from_point_size(saved, window_size);
        if monitors.iter().any(|monitor| monitor.rect.contains(window)) {
            return saved;
        }
    }

    monitors
        .iter()
        .find(|monitor| monitor.primary)
        .or_else(|| monitors.first())
        .map(|monitor| default_bottom_right(monitor.rect, window_size, margin))
        .unwrap_or_default()
}

fn clamp_to_work_area(
    candidate: PhysicalPoint,
    window_size: PhysicalSize,
    work_area: PhysicalRect,
) -> PhysicalPoint {
    let max_x = work_area
        .right
        .saturating_sub(window_size.width)
        .max(work_area.left);
    let max_y = work_area
        .bottom
        .saturating_sub(window_size.height)
        .max(work_area.top);
    PhysicalPoint {
        x: candidate.x.clamp(work_area.left, max_x),
        y: candidate.y.clamp(work_area.top, max_y),
    }
}

fn alpha_hit(alpha: &[u8], width: u32, height: u32, x: i32, y: i32, threshold: u8) -> bool {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return false;
    }
    let index = y as usize * width as usize + x as usize;
    alpha.get(index).is_some_and(|value| *value >= threshold)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GestureOutcome {
    None,
    Click,
    Drag,
}

#[derive(Clone, Copy, Debug)]
struct GestureStart {
    cursor: PhysicalPoint,
    window: PhysicalPoint,
}

#[derive(Default)]
struct PointerGesture {
    start: Option<GestureStart>,
    dragging: bool,
}

impl PointerGesture {
    fn press(&mut self, cursor: PhysicalPoint, window: PhysicalPoint) {
        self.start = Some(GestureStart { cursor, window });
        self.dragging = false;
    }

    fn update(
        &mut self,
        cursor: PhysicalPoint,
        threshold_x: i32,
        threshold_y: i32,
    ) -> Option<PhysicalPoint> {
        let start = self.start?;
        let delta_x = i64::from(cursor.x) - i64::from(start.cursor.x);
        let delta_y = i64::from(cursor.y) - i64::from(start.cursor.y);
        if !self.dragging
            && (delta_x.abs() > i64::from(threshold_x.max(0))
                || delta_y.abs() > i64::from(threshold_y.max(0)))
        {
            self.dragging = true;
        }
        self.dragging.then(|| PhysicalPoint {
            x: saturating_i32(i64::from(start.window.x) + delta_x),
            y: saturating_i32(i64::from(start.window.y) + delta_y),
        })
    }

    fn release(&mut self) -> GestureOutcome {
        if self.start.take().is_none() {
            return GestureOutcome::None;
        }
        let outcome = if self.dragging {
            GestureOutcome::Drag
        } else {
            GestureOutcome::Click
        };
        self.dragging = false;
        outcome
    }

    fn cancel(&mut self) -> GestureOutcome {
        let outcome = if self.start.take().is_some() && self.dragging {
            GestureOutcome::Drag
        } else {
            GestureOutcome::None
        };
        self.dragging = false;
        outcome
    }

    fn is_dragging(&self) -> bool {
        self.dragging
    }
}

fn saturating_i32(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PetState {
    version: u32,
    x: i32,
    y: i32,
}

fn load_state(path: &Path) -> Option<PhysicalPoint> {
    let bytes = std::fs::read(path).ok()?;
    let state: PetState = serde_json::from_slice(&bytes).ok()?;
    (state.version == STATE_VERSION).then_some(PhysicalPoint {
        x: state.x,
        y: state.y,
    })
}

fn save_state(path: &Path, point: PhysicalPoint) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    let state = PetState {
        version: STATE_VERSION,
        x: point.x,
        y: point.y,
    };
    let bytes = serde_json::to_vec(&state).map_err(io::Error::other)?;

    let result = (|| {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        move_file_replacing(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn move_file_replacing(source: &Path, destination: &Path) -> io::Result<()> {
    let source = wide_path(source);
    let destination = wide_path(destination);
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|_| io::Error::last_os_error())
    }
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

struct PetFrame {
    premultiplied_bgra: Vec<u8>,
    alpha: Vec<u8>,
}

struct FrameSet {
    width: i32,
    height: i32,
    frames: Vec<PetFrame>,
}

impl FrameSet {
    fn build(source: &RgbaImage, dpi: u32, size_percent: u32) -> Result<Self, PetStartError> {
        let motion: [(i32, i32); FRAME_COUNT] = [
            (2, 0),
            (1, 1),
            (0, 2),
            (-1, 1),
            (-2, 0),
            (-1, -1),
            (0, -2),
            (1, -1),
        ];
        Self::build_with_motion(source, dpi, size_percent, FilterType::Lanczos3, &motion)
    }

    fn build_preview(
        source: &RgbaImage,
        dpi: u32,
        size_percent: u32,
    ) -> Result<Self, PetStartError> {
        Self::build_with_motion(source, dpi, size_percent, FilterType::Triangle, &[(0, 0)])
    }

    fn build_with_motion(
        source: &RgbaImage,
        dpi: u32,
        size_percent: u32,
        filter: FilterType,
        motion: &[(i32, i32)],
    ) -> Result<Self, PetStartError> {
        let size_percent = normalize_size_percent(i64::from(size_percent));
        let width = scale_for_size(scale_for_dpi(WINDOW_LOGICAL_WIDTH, dpi), size_percent);
        let height = scale_for_size(scale_for_dpi(WINDOW_LOGICAL_HEIGHT, dpi), size_percent);
        let max_pet_width =
            scale_for_size(scale_for_dpi(PET_LOGICAL_WIDTH, dpi), size_percent) as u32;
        let max_pet_height =
            scale_for_size(scale_for_dpi(PET_LOGICAL_HEIGHT, dpi), size_percent) as u32;
        let (base_width, base_height) = fit_size(
            source.width(),
            source.height(),
            max_pet_width,
            max_pet_height,
        );
        let mut frames = Vec::with_capacity(motion.len());

        for &(bob, breath) in motion {
            let frame_height = (base_height as i32
                + scale_signed_for_size(breath, dpi, size_percent))
            .max(1) as u32;
            let resized = image::imageops::resize(source, base_width, frame_height, filter);
            let mut canvas = RgbaImage::new(width as u32, height as u32);
            let x = (width - base_width as i32) / 2;
            let y =
                (height - frame_height as i32) / 2 + scale_signed_for_size(bob, dpi, size_percent);
            image::imageops::overlay(&mut canvas, &resized, i64::from(x), i64::from(y));
            frames.push(convert_frame(canvas));
        }

        Ok(Self {
            width,
            height,
            frames,
        })
    }

    fn size(&self) -> PhysicalSize {
        PhysicalSize {
            width: self.width,
            height: self.height,
        }
    }

    fn hit(&self, frame: usize, x: i32, y: i32) -> bool {
        self.frames.get(frame).is_some_and(|frame| {
            alpha_hit(
                &frame.alpha,
                self.width as u32,
                self.height as u32,
                x,
                y,
                ALPHA_HIT_THRESHOLD,
            )
        })
    }
}

#[cfg(test)]
fn decode_pet_asset() -> Result<RgbaImage, PetStartError> {
    decode_skin_asset(DEFAULT_SKIN_ID)
}

fn resolve_startup_skin(path: &Path) -> String {
    let requested = load_skin_id(path);
    if let Some(skin) = skin_by_id(&requested)
        && decode_skin_asset(skin.id).is_ok()
    {
        return skin.id.into();
    }
    DEFAULT_SKIN_ID.into()
}

fn decode_skin_asset(id: &str) -> Result<RgbaImage, PetStartError> {
    let skin =
        skin_by_id(id).ok_or_else(|| PetStartError(format!("skin {id} is not available")))?;
    decode_skin_asset_from_definition(skin)
}

fn decode_skin_asset_from_definition(skin: SkinDefinition) -> Result<RgbaImage, PetStartError> {
    let source = decode_skin(skin).map_err(|error| PetStartError(error.to_string()))?;
    trim_transparent(&source)
        .ok_or_else(|| PetStartError(format!("{} has no visible pixels", skin.id)))
}

fn trim_transparent(source: &RgbaImage) -> Option<RgbaImage> {
    let mut left = source.width();
    let mut top = source.height();
    let mut right = 0;
    let mut bottom = 0;
    for (x, y, pixel) in source.enumerate_pixels() {
        if pixel[3] == 0 {
            continue;
        }
        left = left.min(x);
        top = top.min(y);
        right = right.max(x + 1);
        bottom = bottom.max(y + 1);
    }
    (right > left && bottom > top).then(|| {
        image::imageops::crop_imm(source, left, top, right - left, bottom - top).to_image()
    })
}

fn fit_size(source_width: u32, source_height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    if u64::from(max_width) * u64::from(source_height)
        <= u64::from(max_height) * u64::from(source_width)
    {
        let height =
            (u64::from(max_width) * u64::from(source_height) / u64::from(source_width)) as u32;
        (max_width.max(1), height.max(1))
    } else {
        let width =
            (u64::from(max_height) * u64::from(source_width) / u64::from(source_height)) as u32;
        (width.max(1), max_height.max(1))
    }
}

fn convert_frame(image: RgbaImage) -> PetFrame {
    let mut premultiplied_bgra = Vec::with_capacity(image.len());
    let mut alpha = Vec::with_capacity(image.width() as usize * image.height() as usize);
    for pixel in image.pixels() {
        let a = u16::from(pixel[3]);
        alpha.push(pixel[3]);
        premultiplied_bgra.push(((u16::from(pixel[2]) * a + 127) / 255) as u8);
        premultiplied_bgra.push(((u16::from(pixel[1]) * a + 127) / 255) as u8);
        premultiplied_bgra.push(((u16::from(pixel[0]) * a + 127) / 255) as u8);
        premultiplied_bgra.push(pixel[3]);
    }
    PetFrame {
        premultiplied_bgra,
        alpha,
    }
}

fn scale_for_dpi(logical: i32, dpi: u32) -> i32 {
    ((i64::from(logical) * i64::from(dpi) + i64::from(BASE_DPI / 2)) / i64::from(BASE_DPI)).max(1)
        as i32
}

fn scale_signed(logical: i32, dpi: u32) -> i32 {
    ((i64::from(logical) * i64::from(dpi)) / i64::from(BASE_DPI)) as i32
}

fn scale_for_size(value: i32, size_percent: u32) -> i32 {
    ((i64::from(value) * i64::from(size_percent) + 50) / 100).max(1) as i32
}

fn scale_signed_for_size(logical: i32, dpi: u32, size_percent: u32) -> i32 {
    let value = i64::from(scale_signed(logical, dpi)) * i64::from(size_percent);
    if value >= 0 {
        ((value + 50) / 100) as i32
    } else {
        ((value - 50) / 100) as i32
    }
}

struct GdiSurface {
    screen: HDC,
    memory: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u8,
    byte_len: usize,
    size: PhysicalSize,
}

impl GdiSurface {
    fn new(size: PhysicalSize) -> Result<Self, PetStartError> {
        unsafe {
            let screen = GetDC(None);
            if screen.0.is_null() {
                return Err(last_windows_error("GetDC"));
            }
            let memory = CreateCompatibleDC(Some(screen));
            if memory.0.is_null() {
                let _ = ReleaseDC(None, screen);
                return Err(last_windows_error("CreateCompatibleDC"));
            }

            let byte_len = size.width as usize * size.height as usize * 4;
            let bitmap_info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size.width,
                    biHeight: -size.height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    biSizeImage: byte_len as u32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = null_mut();
            let bitmap = match CreateDIBSection(
                Some(screen),
                &bitmap_info,
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            ) {
                Ok(bitmap) => bitmap,
                Err(error) => {
                    let _ = DeleteDC(memory);
                    let _ = ReleaseDC(None, screen);
                    return Err(PetStartError(format!("CreateDIBSection failed: {error}")));
                }
            };
            if bits.is_null() {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(memory);
                let _ = ReleaseDC(None, screen);
                return Err(PetStartError(
                    "CreateDIBSection returned no pixel buffer".into(),
                ));
            }
            let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
            if previous.0.is_null() {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                let _ = DeleteDC(memory);
                let _ = ReleaseDC(None, screen);
                return Err(last_windows_error("SelectObject"));
            }

            Ok(Self {
                screen,
                memory,
                bitmap,
                previous,
                bits: bits.cast(),
                byte_len,
                size,
            })
        }
    }

    fn render(&mut self, hwnd: HWND, frame: &PetFrame) -> Result<(), PetStartError> {
        if frame.premultiplied_bgra.len() != self.byte_len {
            return Err(PetStartError(
                "pet frame dimensions do not match GDI surface".into(),
            ));
        }
        unsafe {
            copy_nonoverlapping(frame.premultiplied_bgra.as_ptr(), self.bits, self.byte_len);
            let mut window = RECT::default();
            GetWindowRect(hwnd, &mut window)
                .map_err(|error| PetStartError(format!("GetWindowRect failed: {error}")))?;
            let destination = POINT {
                x: window.left,
                y: window.top,
            };
            let source = POINT::default();
            let size = SIZE {
                cx: self.size.width,
                cy: self.size.height,
            };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            UpdateLayeredWindow(
                hwnd,
                Some(self.screen),
                Some(&destination),
                Some(&size),
                Some(self.memory),
                Some(&source),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
            .map_err(|error| PetStartError(format!("UpdateLayeredWindow failed: {error}")))
        }
    }
}

impl Drop for GdiSurface {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.memory, self.previous);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.memory);
            let _ = ReleaseDC(None, self.screen);
        }
    }
}

struct PetShared {
    hwnd: AtomicIsize,
    thread_id: AtomicU32,
    visible: AtomicBool,
    size_percent: AtomicU32,
    skin_id: Mutex<String>,
    skin_request: Mutex<Option<String>>,
    skin_error: Mutex<Option<(String, String)>>,
    anchor: Mutex<Option<PetAnchor>>,
}

impl Default for PetShared {
    fn default() -> Self {
        Self {
            hwnd: AtomicIsize::new(0),
            thread_id: AtomicU32::new(0),
            visible: AtomicBool::new(false),
            size_percent: AtomicU32::new(100),
            skin_id: Mutex::new(DEFAULT_SKIN_ID.into()),
            skin_request: Mutex::new(None),
            skin_error: Mutex::new(None),
            anchor: Mutex::new(None),
        }
    }
}

struct PetLifetime {
    shared: Arc<PetShared>,
    join: Mutex<Option<JoinHandle<()>>>,
    skin_operation: Mutex<()>,
    shutdown_started: AtomicBool,
}

pub struct PetHandle {
    lifetime: Arc<PetLifetime>,
    preferences_path: PathBuf,
    skin_preferences_path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetSizeUpdate {
    pub size_percent: u32,
    pub saved: bool,
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetSizeStatus {
    pub size_percent: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetSkinUpdate {
    pub skin_id: String,
    pub saved: bool,
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PetSkinStatus {
    pub skin_id: String,
}

#[derive(Debug)]
pub struct PetStartError(String);

impl fmt::Display for PetStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PetStartError {}

impl PetHandle {
    pub fn start<F, A>(
        state_path: PathBuf,
        preferences_path: PathBuf,
        skin_preferences_path: PathBuf,
        on_click: F,
        on_anchor: A,
    ) -> Result<Self, PetStartError>
    where
        F: Fn() + Send + Sync + 'static,
        A: Fn(PetAnchor) + Send + Sync + 'static,
    {
        let shared = Arc::new(PetShared::default());
        let size_percent = load_size_percent(&preferences_path);
        let skin_id = resolve_startup_skin(&skin_preferences_path);
        *shared
            .skin_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = skin_id.clone();
        shared.size_percent.store(size_percent, Ordering::Release);
        let lifetime = Arc::new(PetLifetime {
            shared: shared.clone(),
            join: Mutex::new(None),
            skin_operation: Mutex::new(()),
            shutdown_started: AtomicBool::new(false),
        });
        let (ready, response) = mpsc::sync_channel(1);
        let callback: Arc<dyn Fn() + Send + Sync> = Arc::new(on_click);
        let anchor_callback: Arc<dyn Fn(PetAnchor) + Send + Sync> = Arc::new(on_anchor);
        let join = thread::Builder::new()
            .name("desktop-pet-window".into())
            .spawn(move || {
                pet_thread(
                    shared,
                    state_path,
                    size_percent,
                    skin_id,
                    callback,
                    anchor_callback,
                    ready,
                )
            })
            .map_err(|error| {
                PetStartError(format!("failed to start pet window thread: {error}"))
            })?;
        *lifetime
            .join
            .lock()
            .map_err(|_| PetStartError("pet thread lock is poisoned".into()))? = Some(join);

        match response.recv() {
            Ok(Ok(())) => Ok(Self {
                lifetime,
                preferences_path,
                skin_preferences_path,
            }),
            Ok(Err(error)) => {
                join_pet_thread(&lifetime);
                Err(error)
            }
            Err(_) => {
                join_pet_thread(&lifetime);
                Err(PetStartError(
                    "pet window thread stopped during initialization".into(),
                ))
            }
        }
    }

    pub fn is_visible(&self) -> bool {
        self.lifetime.shared.visible.load(Ordering::Acquire)
    }

    pub fn show(&self) -> bool {
        let sent = self.post(MSG_SHOW);
        if sent {
            self.lifetime.shared.visible.store(true, Ordering::Release);
        }
        sent
    }

    pub fn hide(&self) -> bool {
        let sent = self.post(MSG_HIDE);
        if sent {
            self.lifetime.shared.visible.store(false, Ordering::Release);
        }
        sent
    }

    pub fn ensure_visible(&self) -> bool {
        let sent = self.post(MSG_ENSURE_VISIBLE);
        if sent {
            self.lifetime.shared.visible.store(true, Ordering::Release);
        }
        sent
    }

    pub fn save_position(&self) -> bool {
        self.post(MSG_SAVE_POSITION)
    }

    pub fn size_percent(&self) -> u32 {
        self.lifetime.shared.size_percent.load(Ordering::Acquire)
    }

    pub fn skin_id(&self) -> String {
        self.lifetime
            .shared
            .skin_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn set_size_percent(&self, requested: i64) -> Result<PetSizeUpdate, CommandError> {
        let size_percent = normalize_size_percent(requested);
        let result = self
            .send(MSG_SET_SIZE, WPARAM(size_percent as usize), LPARAM(0))
            .ok_or_else(|| {
                CommandError::new("pet_resize_timeout", "桌宠大小调整超时，请稍后重试。")
            })?;
        if result != 1 {
            return Err(CommandError::new(
                "pet_resize_failed",
                "桌宠大小调整失败，已保留原来的大小。",
            ));
        }

        match save_size_percent(&self.preferences_path, size_percent) {
            Ok(()) => Ok(PetSizeUpdate {
                size_percent,
                saved: true,
                message: None,
            }),
            Err(_) => Ok(PetSizeUpdate {
                size_percent,
                saved: false,
                message: Some("大小已应用，但本地保存失败；重启后将恢复上次保存的大小。".into()),
            }),
        }
    }

    pub fn preview_size_percent(&self, requested: i64) -> Result<PetSizeStatus, CommandError> {
        let size_percent = normalize_size_percent(requested);
        let result = self
            .send(MSG_PREVIEW_SIZE, WPARAM(size_percent as usize), LPARAM(0))
            .ok_or_else(|| {
                CommandError::new("pet_resize_timeout", "桌宠大小预览超时，请稍后重试。")
            })?;
        if result != 1 {
            return Err(CommandError::new(
                "pet_resize_failed",
                "桌宠大小预览失败，已保留原来的大小。",
            ));
        }
        Ok(PetSizeStatus { size_percent })
    }

    pub fn set_skin(&self, requested: &str) -> Result<PetSkinUpdate, CommandError> {
        let _operation = self
            .lifetime
            .skin_operation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let skin = skin_by_id(requested)
            .ok_or_else(|| CommandError::new("pet_skin_unknown_id", "未找到可用的桌宠皮肤。"))?;
        let current = self.skin_id();
        if current == skin.id {
            return Ok(PetSkinUpdate {
                skin_id: current,
                saved: true,
                message: None,
            });
        }

        {
            let mut request = self
                .lifetime
                .shared
                .skin_request
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *request = Some(skin.id.into());
        }
        {
            let mut error = self
                .lifetime
                .shared
                .skin_error
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *error = None;
        }
        let result = self
            .send(MSG_SET_SKIN, WPARAM(0), LPARAM(0))
            .ok_or_else(|| {
                CommandError::new("pet_skin_timeout", "桌宠皮肤切换超时，请稍后重试。")
            })?;
        if result != 1 {
            let (code, message) = self
                .lifetime
                .shared
                .skin_error
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
                .unwrap_or_else(|| {
                    (
                        "pet_skin_frame_failed".into(),
                        "皮肤帧构建失败，已保留原来的外观。".into(),
                    )
                });
            return Err(CommandError::new(code, message));
        }

        match save_skin_id(&self.skin_preferences_path, skin.id) {
            Ok(()) => Ok(PetSkinUpdate {
                skin_id: skin.id.into(),
                saved: true,
                message: None,
            }),
            Err(_) => Ok(PetSkinUpdate {
                skin_id: skin.id.into(),
                saved: false,
                message: Some("皮肤已应用，但本地保存失败；重启后将恢复上次保存的皮肤。".into()),
            }),
        }
    }

    pub fn anchor(&self) -> Result<PetAnchor, CommandError> {
        self.lifetime
            .shared
            .anchor
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .copied()
            .ok_or_else(|| CommandError::new("pet_anchor_unavailable", "桌宠位置暂时不可用。"))
    }

    pub fn shutdown(&self) {
        if self.lifetime.shutdown_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ = self.post(MSG_SHUTDOWN);
        join_pet_thread(&self.lifetime);
    }

    fn post(&self, message: u32) -> bool {
        post_to_shared(&self.lifetime.shared, message)
    }

    fn send(&self, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<usize> {
        let raw = self.lifetime.shared.hwnd.load(Ordering::Acquire);
        if raw == 0 {
            return None;
        }
        let mut result = 0_usize;
        let status = unsafe {
            SendMessageTimeoutW(
                HWND(raw as *mut c_void),
                message,
                wparam,
                lparam,
                SMTO_ABORTIFHUNG,
                MESSAGE_TIMEOUT_MS,
                Some(&mut result),
            )
        };
        (status.0 != 0).then_some(result)
    }
}

impl Drop for PetLifetime {
    fn drop(&mut self) {
        if !self.shutdown_started.swap(true, Ordering::AcqRel) {
            let _ = post_to_shared(&self.shared, MSG_SHUTDOWN);
        }
        if let Ok(mut join) = self.join.lock()
            && let Some(join) = join.take()
        {
            let _ = join.join();
        }
    }
}

fn join_pet_thread(lifetime: &PetLifetime) {
    if let Ok(mut join) = lifetime.join.lock()
        && let Some(join) = join.take()
    {
        let _ = join.join();
    }
}

fn post_to_shared(shared: &PetShared, message: u32) -> bool {
    let raw = shared.hwnd.load(Ordering::Acquire);
    if raw == 0 {
        return false;
    }
    unsafe {
        PostMessageW(
            Some(HWND(raw as *mut c_void)),
            message,
            WPARAM(0),
            LPARAM(0),
        )
        .is_ok()
    }
}

struct NativeWindowState {
    skin_id: String,
    source: RgbaImage,
    frames: FrameSet,
    surface: GdiSurface,
    frame_index: usize,
    dpi: u32,
    size_percent: u32,
    state_path: PathBuf,
    gesture: PointerGesture,
    shared: Arc<PetShared>,
    on_click: Arc<dyn Fn() + Send + Sync>,
    on_anchor: Arc<dyn Fn(PetAnchor) + Send + Sync>,
}

impl NativeWindowState {
    fn new(
        dpi: u32,
        size_percent: u32,
        skin_id: String,
        state_path: PathBuf,
        shared: Arc<PetShared>,
        on_click: Arc<dyn Fn() + Send + Sync>,
        on_anchor: Arc<dyn Fn(PetAnchor) + Send + Sync>,
    ) -> Result<Self, PetStartError> {
        let source = decode_skin_asset(&skin_id)?;
        let frames = FrameSet::build(&source, dpi, size_percent)?;
        let surface = GdiSurface::new(frames.size())?;
        Ok(Self {
            skin_id,
            source,
            frames,
            surface,
            frame_index: 0,
            dpi,
            size_percent,
            state_path,
            gesture: PointerGesture::default(),
            shared,
            on_click,
            on_anchor,
        })
    }

    fn render(&mut self, hwnd: HWND) -> Result<(), PetStartError> {
        self.surface
            .render(hwnd, &self.frames.frames[self.frame_index])
    }

    fn advance_animation(&mut self, hwnd: HWND) {
        if !self.shared.visible.load(Ordering::Acquire) || self.gesture.is_dragging() {
            return;
        }
        self.frame_index = (self.frame_index + 1) % self.frames.frames.len();
        let _ = self.render(hwnd);
    }

    fn show(&mut self, hwnd: HWND) {
        self.ensure_visible(hwnd);
        let _ = self.render(hwnd);
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        self.shared.visible.store(true, Ordering::Release);
        self.notify_anchor(hwnd);
    }

    fn hide(&mut self, hwnd: HWND) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
        self.shared.visible.store(false, Ordering::Release);
        self.notify_anchor(hwnd);
    }

    fn ensure_visible(&mut self, hwnd: HWND) {
        let Some(current) = window_point(hwnd) else {
            return;
        };
        let Ok(monitors) = enumerate_work_areas() else {
            return;
        };
        let corrected = restore_position(
            Some(current),
            &monitors,
            self.frames.size(),
            scale_for_dpi(DEFAULT_MARGIN_LOGICAL, self.dpi),
        );
        if corrected != current {
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    corrected.x,
                    corrected.y,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
            let _ = save_state(&self.state_path, corrected);
        }
    }

    fn save_position(&self, hwnd: HWND) {
        if let Some(point) = window_point(hwnd) {
            let _ = save_state(&self.state_path, point);
        }
    }

    fn mouse_down(&mut self, hwnd: HWND) {
        let (Some(cursor), Some(window)) = (cursor_point(), window_point(hwnd)) else {
            return;
        };
        self.gesture.press(cursor, window);
        unsafe {
            let _ = SetCapture(hwnd);
        }
    }

    fn mouse_move(&mut self, hwnd: HWND) {
        let Some(cursor) = cursor_point() else {
            return;
        };
        let (threshold_x, threshold_y) = unsafe {
            (
                GetSystemMetrics(SM_CXDRAG).max(1),
                GetSystemMetrics(SM_CYDRAG).max(1),
            )
        };
        let Some(candidate) = self.gesture.update(cursor, threshold_x, threshold_y) else {
            return;
        };
        let corrected =
            clamp_to_nearest_monitor(candidate, self.frames.size()).unwrap_or(candidate);
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                corrected.x,
                corrected.y,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        self.notify_anchor(hwnd);
    }

    fn mouse_up(&mut self, hwnd: HWND) {
        let outcome = self.gesture.release();
        unsafe {
            let _ = ReleaseCapture();
        }
        match outcome {
            GestureOutcome::Click => {
                (self.on_click)();
            }
            GestureOutcome::Drag => {
                self.ensure_visible(hwnd);
                self.save_position(hwnd);
                self.notify_anchor(hwnd);
            }
            GestureOutcome::None => {}
        }
    }

    fn capture_changed(&mut self, hwnd: HWND) {
        if self.gesture.cancel() == GestureOutcome::Drag {
            self.ensure_visible(hwnd);
            self.save_position(hwnd);
            self.notify_anchor(hwnd);
        }
    }

    fn change_dpi(&mut self, hwnd: HWND, dpi: u32, suggested: PhysicalRect) {
        let dpi = dpi.max(BASE_DPI);
        let candidate = PhysicalPoint {
            x: suggested.left,
            y: suggested.top,
        };
        let _ = self.rebuild_frames(hwnd, dpi, self.size_percent, candidate);
    }

    fn change_size(&mut self, hwnd: HWND, requested: i64) -> Result<(), PetStartError> {
        let size_percent = normalize_size_percent(requested);
        let candidate = window_point(hwnd).unwrap_or_default();
        self.rebuild_frames(hwnd, self.dpi, size_percent, candidate)
    }

    fn preview_size(&mut self, hwnd: HWND, requested: i64) -> Result<(), PetStartError> {
        let size_percent = normalize_size_percent(requested);
        let candidate = window_point(hwnd).unwrap_or_default();
        let frames = FrameSet::build_preview(&self.source, self.dpi, size_percent)?;
        self.replace_frames(hwnd, frames, self.dpi, size_percent, candidate)
    }

    fn change_skin(&mut self, hwnd: HWND) -> Result<(), (String, String)> {
        let requested = self
            .shared
            .skin_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(requested) = requested else {
            return Err((
                "pet_skin_unknown_id".into(),
                "未找到可用的桌宠皮肤。".into(),
            ));
        };
        let skin = skin_by_id(&requested).ok_or_else(|| {
            (
                "pet_skin_unknown_id".into(),
                "未找到可用的桌宠皮肤。".into(),
            )
        })?;
        let source = decode_skin_asset_from_definition(skin).map_err(|error| {
            (
                "pet_skin_resource_failed".into(),
                format!("皮肤资源加载失败，已保留原来的外观：{error}"),
            )
        })?;
        let frames = FrameSet::build(&source, self.dpi, self.size_percent).map_err(|error| {
            (
                "pet_skin_frame_failed".into(),
                format!("皮肤帧构建失败，已保留原来的外观：{error}"),
            )
        })?;
        let candidate = window_point(hwnd).unwrap_or_default();
        let old_skin_id = self.skin_id.clone();
        if let Err(error) =
            self.replace_frames(hwnd, frames, self.dpi, self.size_percent, candidate)
        {
            self.skin_id = old_skin_id;
            return Err((
                "pet_skin_frame_failed".into(),
                format!("皮肤切换失败，已保留原来的外观：{error}"),
            ));
        }
        self.source = source;
        self.skin_id = skin.id.into();
        *self
            .shared
            .skin_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = self.skin_id.clone();
        Ok(())
    }

    fn rebuild_frames(
        &mut self,
        hwnd: HWND,
        dpi: u32,
        size_percent: u32,
        candidate: PhysicalPoint,
    ) -> Result<(), PetStartError> {
        let frames = FrameSet::build(&self.source, dpi, size_percent)?;
        self.replace_frames(hwnd, frames, dpi, size_percent, candidate)
    }

    fn replace_frames(
        &mut self,
        hwnd: HWND,
        frames: FrameSet,
        dpi: u32,
        size_percent: u32,
        candidate: PhysicalPoint,
    ) -> Result<(), PetStartError> {
        let mut surface = GdiSurface::new(frames.size())?;
        let corrected = clamp_to_nearest_monitor(candidate, frames.size()).unwrap_or(candidate);
        let old_point = window_point(hwnd).unwrap_or(candidate);
        unsafe {
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                corrected.x,
                corrected.y,
                frames.width,
                frames.height,
                SWP_NOACTIVATE,
            )
            .map_err(|error| PetStartError(format!("SetWindowPos failed: {error}")))?;
        }
        if let Err(error) = surface.render(hwnd, &frames.frames[0]) {
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    old_point.x,
                    old_point.y,
                    self.frames.width,
                    self.frames.height,
                    SWP_NOACTIVATE,
                );
            }
            let _ = self.render(hwnd);
            return Err(error);
        }

        self.frames = frames;
        self.surface = surface;
        self.frame_index = 0;
        self.dpi = dpi;
        self.size_percent = size_percent;
        self.shared
            .size_percent
            .store(size_percent, Ordering::Release);
        self.save_position(hwnd);
        self.notify_anchor(hwnd);
        Ok(())
    }

    fn anchor(&self, hwnd: HWND) -> Option<PetAnchor> {
        let mut rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut rect).ok()? };
        let monitor = unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST) };
        let work_area = monitor_work_area(monitor)?.rect;
        Some(PetAnchor {
            pet_rect: AnchorRect {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            },
            work_area: AnchorRect {
                left: work_area.left,
                top: work_area.top,
                right: work_area.right,
                bottom: work_area.bottom,
            },
            dpi: self.dpi,
            visible: self.shared.visible.load(Ordering::Acquire),
        })
    }

    fn notify_anchor(&self, hwnd: HWND) {
        if let Some(anchor) = self.anchor(hwnd) {
            *self
                .shared
                .anchor
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(anchor);
            (self.on_anchor)(anchor);
        }
    }
}

fn pet_thread(
    shared: Arc<PetShared>,
    state_path: PathBuf,
    size_percent: u32,
    skin_id: String,
    on_click: Arc<dyn Fn() + Send + Sync>,
    on_anchor: Arc<dyn Fn(PetAnchor) + Send + Sync>,
    ready: mpsc::SyncSender<Result<(), PetStartError>>,
) {
    shared
        .thread_id
        .store(unsafe { GetCurrentThreadId() }, Ordering::Release);
    let result = unsafe {
        initialize_and_run(
            shared.clone(),
            state_path,
            size_percent,
            skin_id,
            on_click,
            on_anchor,
            &ready,
        )
    };
    if let Err(error) = result {
        let _ = ready.send(Err(error));
    }
    shared.visible.store(false, Ordering::Release);
    shared.hwnd.store(0, Ordering::Release);
    shared.thread_id.store(0, Ordering::Release);
}

unsafe fn initialize_and_run(
    shared: Arc<PetShared>,
    state_path: PathBuf,
    size_percent: u32,
    skin_id: String,
    on_click: Arc<dyn Fn() + Send + Sync>,
    on_anchor: Arc<dyn Fn(PetAnchor) + Send + Sync>,
    ready: &mpsc::SyncSender<Result<(), PetStartError>>,
) -> Result<(), PetStartError> {
    let dpi = unsafe { GetDpiForSystem() }.max(BASE_DPI);
    let mut state = Box::new(NativeWindowState::new(
        dpi,
        size_percent,
        skin_id,
        state_path,
        shared.clone(),
        on_click,
        on_anchor,
    )?);
    let monitors = enumerate_work_areas()?;
    let position = restore_position(
        load_state(&state.state_path),
        &monitors,
        state.frames.size(),
        scale_for_dpi(DEFAULT_MARGIN_LOGICAL, dpi),
    );
    let module = unsafe { GetModuleHandleW(None) }
        .map_err(|error| PetStartError(format!("GetModuleHandleW failed: {error}")))?;
    let instance = HINSTANCE(module.0);
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }
        .map_err(|error| PetStartError(format!("LoadCursorW failed: {error}")))?;
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: cursor,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&class) } == 0 {
        return Err(last_windows_error("RegisterClassExW"));
    }
    let extended_style = WINDOW_EX_STYLE(
        WS_EX_LAYERED.0 | WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0 | WS_EX_NOACTIVATE.0,
    );
    let hwnd = match unsafe {
        CreateWindowExW(
            extended_style,
            CLASS_NAME,
            w!("小桌宠"),
            WS_POPUP,
            position.x,
            position.y,
            state.frames.width,
            state.frames.height,
            None,
            None,
            Some(instance),
            Some(state.as_mut() as *mut NativeWindowState as *const c_void),
        )
    } {
        Ok(hwnd) => hwnd,
        Err(error) => {
            let _ = unsafe { UnregisterClassW(CLASS_NAME, Some(instance)) };
            return Err(PetStartError(format!("CreateWindowExW failed: {error}")));
        }
    };
    shared.hwnd.store(hwnd.0 as isize, Ordering::Release);

    if let Err(error) = state.render(hwnd) {
        let _ = unsafe { DestroyWindow(hwnd) };
        let _ = unsafe { UnregisterClassW(CLASS_NAME, Some(instance)) };
        return Err(error);
    }
    if unsafe { SetTimer(Some(hwnd), TIMER_ID, TIMER_INTERVAL_MS, None) } == 0 {
        let error = last_windows_error("SetTimer");
        let _ = unsafe { DestroyWindow(hwnd) };
        let _ = unsafe { UnregisterClassW(CLASS_NAME, Some(instance)) };
        return Err(error);
    }
    state.show(hwnd);
    if ready.send(Ok(())).is_err() {
        let _ = unsafe { DestroyWindow(hwnd) };
        let _ = unsafe { UnregisterClassW(CLASS_NAME, Some(instance)) };
        return Err(PetStartError(
            "application stopped while pet window was starting".into(),
        ));
    }

    let mut message = MSG::default();
    loop {
        let status = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if status.0 == -1 {
            break;
        }
        if !status.as_bool() {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        let _ = unsafe { DestroyWindow(hwnd) };
    }
    let _ = unsafe { KillTimer(Some(hwnd), TIMER_ID) };
    let _ = unsafe { UnregisterClassW(CLASS_NAME, Some(instance)) };
    drop(state);
    Ok(())
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    std::panic::catch_unwind(|| unsafe { window_proc_inner(hwnd, message, wparam, lparam) })
        .unwrap_or_else(|_| unsafe { DefWindowProcW(hwnd, message, wparam, lparam) })
}

unsafe fn window_proc_inner(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
    }

    let state_pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut NativeWindowState;
    if !state_pointer.is_null() {
        let state = unsafe { &mut *state_pointer };
        match message {
            WM_NCHITTEST => {
                let mut point = point_from_lparam(lparam);
                if !unsafe { ScreenToClient(hwnd, &mut point) }.as_bool() {
                    return LRESULT(HTTRANSPARENT as isize);
                }
                return if state.frames.hit(state.frame_index, point.x, point.y) {
                    LRESULT(HTCLIENT as isize)
                } else {
                    LRESULT(HTTRANSPARENT as isize)
                };
            }
            WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
            WM_LBUTTONDOWN => {
                state.mouse_down(hwnd);
                return LRESULT(0);
            }
            WM_MOUSEMOVE => {
                state.mouse_move(hwnd);
                return LRESULT(0);
            }
            WM_LBUTTONUP => {
                state.mouse_up(hwnd);
                return LRESULT(0);
            }
            WM_CAPTURECHANGED => {
                state.capture_changed(hwnd);
                return LRESULT(0);
            }
            WM_TIMER if wparam.0 == TIMER_ID => {
                state.advance_animation(hwnd);
                return LRESULT(0);
            }
            WM_DPICHANGED => {
                let dpi = (wparam.0 as u32) & 0xffff;
                let suggested = PhysicalRect::from(unsafe { *(lparam.0 as *const RECT) });
                state.change_dpi(hwnd, dpi, suggested);
                return LRESULT(0);
            }
            WM_DISPLAYCHANGE => {
                state.ensure_visible(hwnd);
                let _ = state.render(hwnd);
                state.save_position(hwnd);
                state.notify_anchor(hwnd);
                return LRESULT(0);
            }
            MSG_SHOW | MSG_ENSURE_VISIBLE => {
                state.show(hwnd);
                return LRESULT(0);
            }
            MSG_HIDE => {
                state.hide(hwnd);
                return LRESULT(0);
            }
            MSG_SAVE_POSITION => {
                state.save_position(hwnd);
                return LRESULT(0);
            }
            MSG_SET_SIZE => {
                return if state.change_size(hwnd, wparam.0 as i64).is_ok() {
                    LRESULT(1)
                } else {
                    LRESULT(0)
                };
            }
            MSG_PREVIEW_SIZE => {
                return if state.preview_size(hwnd, wparam.0 as i64).is_ok() {
                    LRESULT(1)
                } else {
                    LRESULT(0)
                };
            }
            MSG_SET_SKIN => {
                let result = state.change_skin(hwnd);
                if let Err((code, message)) = result {
                    *state
                        .shared
                        .skin_error
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((code, message));
                    return LRESULT(0);
                }
                return LRESULT(1);
            }
            MSG_SHUTDOWN => {
                state.save_position(hwnd);
                let _ = unsafe { DestroyWindow(hwnd) };
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_CLOSE => {
                state.hide(hwnd);
                return LRESULT(0);
            }
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
                return LRESULT(0);
            }
            WM_NCDESTROY => unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            },
            _ => {}
        }
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn point_from_lparam(lparam: LPARAM) -> POINT {
    POINT {
        x: (lparam.0 as u16) as i16 as i32,
        y: ((lparam.0 >> 16) as u16) as i16 as i32,
    }
}

fn cursor_point() -> Option<PhysicalPoint> {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point).ok()? };
    Some(PhysicalPoint {
        x: point.x,
        y: point.y,
    })
}

fn window_point(hwnd: HWND) -> Option<PhysicalPoint> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect).ok()? };
    Some(PhysicalPoint {
        x: rect.left,
        y: rect.top,
    })
}

fn clamp_to_nearest_monitor(
    candidate: PhysicalPoint,
    window_size: PhysicalSize,
) -> Option<PhysicalPoint> {
    let rect = PhysicalRect::from_point_size(candidate, window_size);
    let native_rect = RECT::from(rect);
    let monitor = unsafe { MonitorFromRect(&native_rect, MONITOR_DEFAULTTONEAREST) };
    monitor_work_area(monitor)
        .map(|work_area| clamp_to_work_area(candidate, window_size, work_area.rect))
}

fn enumerate_work_areas() -> Result<Vec<MonitorWorkArea>, PetStartError> {
    let mut monitors = Vec::new();
    let succeeded = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM((&mut monitors as *mut Vec<MonitorWorkArea>) as isize),
        )
    };
    if !succeeded.as_bool() || monitors.is_empty() {
        return Err(last_windows_error("EnumDisplayMonitors"));
    }
    Ok(monitors)
}

unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let monitors = unsafe { &mut *(data.0 as *mut Vec<MonitorWorkArea>) };
    if let Some(work_area) = monitor_work_area(monitor) {
        monitors.push(work_area);
    }
    BOOL(1)
}

fn monitor_work_area(monitor: HMONITOR) -> Option<MonitorWorkArea> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return None;
    }
    Some(MonitorWorkArea {
        rect: PhysicalRect::from(info.rcWork),
        primary: info.dwFlags & 1 != 0,
    })
}

fn last_windows_error(operation: &str) -> PetStartError {
    PetStartError(format!(
        "{operation} failed: {}",
        WindowsError::from_thread()
    ))
}

#[cfg(test)]
mod tests {
    use crate::pet_skins::available_skins;
    use tempfile::tempdir;

    use super::*;

    fn desktop_monitors() -> Vec<MonitorWorkArea> {
        vec![
            MonitorWorkArea {
                rect: PhysicalRect {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1040,
                },
                primary: true,
            },
            MonitorWorkArea {
                rect: PhysicalRect {
                    left: -1280,
                    top: 0,
                    right: 0,
                    bottom: 984,
                },
                primary: false,
            },
        ]
    }

    #[test]
    fn default_position_uses_primary_bottom_right_with_margin() {
        let point = restore_position(
            None,
            &desktop_monitors(),
            PhysicalSize {
                width: 220,
                height: 240,
            },
            24,
        );
        assert_eq!(point, PhysicalPoint { x: 1676, y: 776 });
    }

    #[test]
    fn valid_saved_position_is_restored_on_any_monitor() {
        let saved = PhysicalPoint { x: -900, y: 300 };
        let restored = restore_position(
            Some(saved),
            &desktop_monitors(),
            PhysicalSize {
                width: 220,
                height: 240,
            },
            24,
        );
        assert_eq!(restored, saved);
    }

    #[test]
    fn offscreen_saved_position_falls_back_to_primary() {
        let restored = restore_position(
            Some(PhysicalPoint { x: 4000, y: 3000 }),
            &desktop_monitors(),
            PhysicalSize {
                width: 220,
                height: 240,
            },
            24,
        );
        assert_eq!(restored, PhysicalPoint { x: 1676, y: 776 });
    }

    #[test]
    fn clamp_keeps_the_complete_window_inside_work_area() {
        let work_area = desktop_monitors()[0].rect;
        let size = PhysicalSize {
            width: 220,
            height: 240,
        };
        assert_eq!(
            clamp_to_work_area(PhysicalPoint { x: -50, y: -20 }, size, work_area),
            PhysicalPoint { x: 0, y: 0 }
        );
        assert_eq!(
            clamp_to_work_area(PhysicalPoint { x: 1900, y: 1000 }, size, work_area),
            PhysicalPoint { x: 1700, y: 800 }
        );
    }

    #[test]
    fn alpha_hit_rejects_outside_and_transparent_pixels() {
        let alpha = [0, 31, 32, 255];
        assert!(!alpha_hit(&alpha, 2, 2, -1, 0, 32));
        assert!(!alpha_hit(&alpha, 2, 2, 2, 0, 32));
        assert!(!alpha_hit(&alpha, 2, 2, 0, 0, 32));
        assert!(!alpha_hit(&alpha, 2, 2, 1, 0, 32));
        assert!(alpha_hit(&alpha, 2, 2, 0, 1, 32));
        assert!(alpha_hit(&alpha, 2, 2, 1, 1, 32));
    }

    #[test]
    fn gesture_classifies_short_movement_as_click() {
        let mut gesture = PointerGesture::default();
        gesture.press(
            PhysicalPoint { x: 100, y: 100 },
            PhysicalPoint { x: 500, y: 300 },
        );
        assert_eq!(gesture.update(PhysicalPoint { x: 103, y: 104 }, 4, 4), None);
        assert_eq!(gesture.release(), GestureOutcome::Click);
    }

    #[test]
    fn gesture_drag_uses_system_threshold_shape_and_never_clicks_on_release() {
        let mut gesture = PointerGesture::default();
        gesture.press(
            PhysicalPoint { x: 100, y: 100 },
            PhysicalPoint { x: 500, y: 300 },
        );
        assert_eq!(
            gesture.update(PhysicalPoint { x: 106, y: 102 }, 4, 4),
            Some(PhysicalPoint { x: 506, y: 302 })
        );
        assert_eq!(gesture.release(), GestureOutcome::Drag);
        assert_eq!(gesture.release(), GestureOutcome::None);
    }

    #[test]
    fn state_round_trips_atomically_and_corruption_falls_back() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("pet-state.json");
        assert_eq!(load_state(&path), None);

        save_state(&path, PhysicalPoint { x: -320, y: 480 }).unwrap();
        assert_eq!(load_state(&path), Some(PhysicalPoint { x: -320, y: 480 }));
        save_state(&path, PhysicalPoint { x: 60, y: 90 }).unwrap();
        assert_eq!(load_state(&path), Some(PhysicalPoint { x: 60, y: 90 }));
        assert!(!path.with_extension("json.tmp").exists());

        std::fs::write(&path, b"not-json").unwrap();
        assert_eq!(load_state(&path), None);
        std::fs::write(&path, br#"{"version":99,"x":1,"y":2}"#).unwrap();
        assert_eq!(load_state(&path), None);
    }

    #[test]
    fn startup_skin_recovery_is_deterministic_without_creating_a_window() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("pet-skin-preferences.json");
        assert_eq!(resolve_startup_skin(&path), DEFAULT_SKIN_ID);

        save_skin_id(&path, "calico-cat").unwrap();
        assert_eq!(resolve_startup_skin(&path), "calico-cat");

        std::fs::write(&path, br#"{"version":99,"skin_id":"orange-dragon"}"#).unwrap();
        assert_eq!(resolve_startup_skin(&path), DEFAULT_SKIN_ID);
        std::fs::write(&path, br#"{"version":1,"skin_id":"unknown"}"#).unwrap();
        assert_eq!(resolve_startup_skin(&path), DEFAULT_SKIN_ID);
    }

    #[test]
    fn embedded_png_builds_premultiplied_idle_frames() {
        let source = decode_pet_asset().unwrap();
        let frames = FrameSet::build(&source, BASE_DPI, 100).unwrap();
        assert_eq!(frames.frames.len(), FRAME_COUNT);
        assert_eq!(
            frames.size(),
            PhysicalSize {
                width: 220,
                height: 240
            }
        );
        assert!(frames.frames[0].alpha.contains(&0));
        assert!(frames.frames[0].alpha.contains(&255));
        for pixel in frames.frames[0].premultiplied_bgra.chunks_exact(4) {
            assert!(pixel[0] <= pixel[3]);
            assert!(pixel[1] <= pixel[3]);
            assert!(pixel[2] <= pixel[3]);
        }
    }

    #[test]
    fn preview_builds_one_aligned_frame_at_the_requested_size() {
        let source = decode_pet_asset().unwrap();
        let frames = FrameSet::build_preview(&source, BASE_DPI, 30).unwrap();
        assert_eq!(frames.frames.len(), 1);
        assert_eq!(
            frames.size(),
            PhysicalSize {
                width: 66,
                height: 72
            }
        );
        assert_eq!(
            frames.frames[0].alpha.len(),
            frames.width as usize * frames.height as usize
        );
        assert_eq!(
            frames.frames[0].premultiplied_bgra.len(),
            frames.frames[0].alpha.len() * 4
        );
    }

    #[test]
    fn dpi_scaling_is_stable_for_supported_acceptance_scales() {
        assert_eq!(scale_for_dpi(WINDOW_LOGICAL_WIDTH, 96), 220);
        assert_eq!(scale_for_dpi(WINDOW_LOGICAL_WIDTH, 120), 275);
        assert_eq!(scale_for_dpi(WINDOW_LOGICAL_WIDTH, 144), 330);
    }

    #[test]
    fn pet_size_is_applied_after_dpi_and_keeps_frames_and_hits_aligned() {
        let source = decode_pet_asset().unwrap();
        for (size_percent, expected) in [
            (
                30,
                PhysicalSize {
                    width: 66,
                    height: 72,
                },
            ),
            (
                100,
                PhysicalSize {
                    width: 220,
                    height: 240,
                },
            ),
            (
                160,
                PhysicalSize {
                    width: 352,
                    height: 384,
                },
            ),
        ] {
            let frames = FrameSet::build(&source, BASE_DPI, size_percent).unwrap();
            assert_eq!(frames.size(), expected);
            assert_eq!(frames.frames.len(), FRAME_COUNT);
            for frame in &frames.frames {
                assert_eq!(
                    frame.alpha.len(),
                    expected.width as usize * expected.height as usize
                );
                assert_eq!(frame.premultiplied_bgra.len(), frame.alpha.len() * 4);
                for pixel in frame.premultiplied_bgra.chunks_exact(4) {
                    assert!(pixel[0] <= pixel[3]);
                    assert!(pixel[1] <= pixel[3]);
                    assert!(pixel[2] <= pixel[3]);
                }
            }
        }

        let scaled = FrameSet::build(&source, 120, 130).unwrap();
        assert_eq!(
            scaled.size(),
            PhysicalSize {
                width: 358,
                height: 390
            }
        );
    }

    #[test]
    fn every_builtin_skin_keeps_frame_and_hit_buffers_aligned_at_supported_scales_and_dpi() {
        for skin in available_skins() {
            let source = decode_skin_asset(skin.id).unwrap();
            for dpi in [96, 120, 144] {
                for size_percent in [30, 70, 100, 130, 160] {
                    let frames = FrameSet::build(&source, dpi, size_percent).unwrap();
                    assert_eq!(frames.frames.len(), FRAME_COUNT);
                    assert!(frames.width > 0 && frames.height > 0);
                    for frame in &frames.frames {
                        assert_eq!(
                            frame.alpha.len(),
                            frames.width as usize * frames.height as usize
                        );
                        assert_eq!(frame.premultiplied_bgra.len(), frame.alpha.len() * 4);
                        assert!(frame.alpha.iter().any(|alpha| *alpha == 0));
                        assert!(
                            frame
                                .alpha
                                .iter()
                                .any(|alpha| *alpha >= ALPHA_HIT_THRESHOLD)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn an_unavailable_skin_candidate_fails_without_changing_the_previous_id() {
        let previous = String::from(DEFAULT_SKIN_ID);
        let result = decode_skin_asset("missing-skin");
        assert!(result.is_err());
        assert_eq!(previous, DEFAULT_SKIN_ID);
    }

    #[test]
    fn enlarged_pet_is_clamped_fully_inside_the_selected_work_area() {
        let work_area = desktop_monitors()[0].rect;
        let enlarged = PhysicalSize {
            width: 352,
            height: 384,
        };
        assert_eq!(
            clamp_to_work_area(PhysicalPoint { x: 1676, y: 776 }, enlarged, work_area),
            PhysicalPoint { x: 1568, y: 656 }
        );
        let secondary = desktop_monitors()[1].rect;
        assert_eq!(
            clamp_to_work_area(PhysicalPoint { x: -50, y: 800 }, enlarged, secondary),
            PhysicalPoint { x: -352, y: 600 }
        );
    }
}
