#!/usr/bin/env bash
# Capture 1200×800 screenshots of Bambu Studio / OrcaSlicer / PrusaSlicer for
# crates/bambu-ui/tests/gui/upstream. Does not launch iced.
#
#   UPDATE_GUI_UPSTREAM=1 ./scripts/capture-upstream-gui.sh
#   UPDATE_GUI_UPSTREAM=1 UPSTREAM_CAPTURE=orca ./scripts/capture-upstream-gui.sh
#
# Uses a throwaway HOME, software GL (Xvfb has no DRI3), ImageMagick import,
# and xdotool tab clicks when available. Tiny/black grabs are discarded.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${ROOT}/crates/bambu-ui/tests/gui/upstream"
W=1200
H=800
WAIT="${UPSTREAM_GUI_WAIT:-18}"
MIN_BYTES="${UPSTREAM_MIN_BYTES:-20000}"
USER_HOME="$(getent passwd "$(id -un)" | cut -d: -f6)"
USER_HOME="${USER_HOME:-/home/luluco}"

if [[ "${UPDATE_GUI_UPSTREAM:-}" != "1" && "${UPDATE_GUI_UPSTREAM:-}" != "true" ]]; then
  echo "set UPDATE_GUI_UPSTREAM=1 to capture (refusing to launch C++ GUIs)" >&2
  exit 1
fi

add_nix_bin() {
  local name="$1"
  if command -v "$name" >/dev/null 2>&1; then
    return 0
  fi
  local p
  for p in /nix/store/*/"bin/${name}"; do
    if [[ -x "$p" ]]; then
      PATH="$(dirname "$p"):$PATH"
      export PATH
      return 0
    fi
  done
  return 1
}

ensure_import() {
  add_nix_bin import || {
    echo "need ImageMagick import (or scrot/grim) to grab the framebuffer" >&2
    return 1
  }
  add_nix_bin magick || add_nix_bin convert || true
}

ensure_xvfb() {
  command -v Xvfb >/dev/null 2>&1 || {
    echo "Xvfb not found" >&2
    return 1
  }
}

grab_root() {
  local dest="$1"
  if command -v import >/dev/null 2>&1; then
    import -window root "${dest}"
  elif command -v scrot >/dev/null 2>&1; then
    scrot "${dest}"
  elif command -v grim >/dev/null 2>&1; then
    grim "${dest}"
  else
    return 1
  fi
}

find_bin() {
  local env_name="$1"
  shift
  if [[ -n "${!env_name:-}" && -x "${!env_name}" ]]; then
    printf '%s\n' "${!env_name}"
    return 0
  fi
  local n
  for n in "$@"; do
    if command -v "$n" >/dev/null 2>&1; then
      command -v "$n"
      return 0
    fi
  done
  return 1
}

to_png32() {
  local dest="$1"
  if command -v magick >/dev/null 2>&1; then
    magick "${dest}" PNG32:"${dest}.tmp" && mv "${dest}.tmp" "${dest}"
  elif command -v convert >/dev/null 2>&1; then
    convert "${dest}" PNG32:"${dest}.tmp" && mv "${dest}.tmp" "${dest}"
  fi
}

mean_luma() {
  local dest="$1"
  if command -v magick >/dev/null 2>&1; then
    magick "${dest}" -colorspace Gray -format "%[fx:mean*255]" info:
  else
    echo 255
  fi
}

keep_grab() {
  local dest="$1"
  local bytes luma
  bytes="$(wc -c < "${dest}")"
  if (( bytes < MIN_BYTES )); then
    echo "discard ${dest}: ${bytes} bytes (empty Xvfb grab)" >&2
    rm -f "${dest}"
    return 1
  fi
  luma="$(mean_luma "${dest}" || echo 0)"
  luma="${luma%.*}"
  if [[ -z "${luma}" ]]; then
    luma=0
  fi
  if (( luma < 12 )); then
    echo "discard ${dest}: mean luma ${luma} (black Xvfb grab)" >&2
    rm -f "${dest}"
    return 1
  fi
  to_png32 "${dest}" || true
  echo "wrote ${dest} (${bytes} bytes, luma ${luma})" >&2
}

rsync_user_presets() {
  local name="$1"
  local dest="$2"
  local src="${USER_HOME}/.config/${name}"
  [[ -d "${src}" ]] || return 1
  mkdir -p "${dest}"
  local sub
  for sub in system printers user; do
    if [[ -d "${src}/${sub}" ]]; then
      rsync -a "${src}/${sub}/" "${dest}/${sub}/"
    fi
  done
  echo "seeded ${name} presets from ${src}" >&2
}

write_bambu_conf() {
  local dir="$1"
  mkdir -p "${dir}"
  cat >"${dir}/BambuStudio.conf" <<'EOF'
{
    "app": {
        "dark_color_mode": "1",
        "enable_beta_version_update": false,
        "iot_environment": "3",
        "language": "en",
        "region": "North America",
        "show_daily_tips": false,
        "show_home_page": true,
        "show_hints": false,
        "single_instance": false,
        "sync_system_preset": false,
        "units": "0",
        "window_mainframe": "0; 0; 1200; 800; 0"
    },
    "firstguide": {
        "finish": "1",
        "privacyuse": "true"
    }
}
EOF
}

write_orca_conf() {
  local dir="$1"
  mkdir -p "${dir}"
  cat >"${dir}/OrcaSlicer.conf" <<'EOF'
{
    "app": {
        "dark_color_mode": "1",
        "enable_beta_version_update": false,
        "language": "en",
        "show_daily_tips": false,
        "show_hints": false,
        "single_instance": false,
        "window_mainframe": "0; 0; 1200; 800; 0"
    },
    "firstguide": {
        "finish": "1",
        "privacyuse": "true"
    }
}
EOF
}

write_prusa_ini() {
  local dir="$1"
  mkdir -p "${dir}"
  cat >"${dir}/PrusaSlicer.ini" <<'EOF'
dark_color_mode = 1
version_check = 0
auto_mint = 0
show_hints = 0
EOF
}

seed_family() {
  local family="$1"
  local home="$2"
  case "${family}" in
    bambu)
      write_bambu_conf "${home}/.config/BambuStudio"
      rsync_user_presets BambuStudio "${home}/.config/BambuStudio" || true
      ;;
    orca)
      write_orca_conf "${home}/.config/OrcaSlicer"
      rsync_user_presets OrcaSlicer "${home}/.config/OrcaSlicer" || true
      ;;
    prusa)
      write_prusa_ini "${home}/.config/PrusaSlicer"
      write_prusa_ini "${home}"
      ;;
  esac
}

click_xy() {
  local disp="$1"
  local x="$2"
  local y="$3"
  DISPLAY="${disp}" xdotool mousemove "$x" "$y"
  sleep 0.15
  DISPLAY="${disp}" xdotool click 1
}

grab_named() {
  local disp="$1"
  local dest="$2"
  DISPLAY="${disp}" grab_root "${dest}" || return 0
  keep_grab "${dest}" || true
}

pngs_identical() {
  local a="$1"
  local b="$2"
  [[ -f "${a}" && -f "${b}" ]] || return 1
  if command -v magick >/dev/null 2>&1; then
    local ae
    ae="$(magick compare -metric AE "${a}" "${b}" /tmp/bambu-upstream-ae.png 2>&1 || true)"
    ae="${ae%% *}"
    [[ "${ae}" == "0" ]]
  else
    cmp -s "${a}" "${b}"
  fi
}

drop_duplicate_tabs() {
  local dir="$1"
  local base="$2"
  local f
  [[ -f "${dir}/${base}" ]] || return 0
  for f in "${dir}"/*.png; do
    [[ -f "${f}" ]] || continue
    [[ "$(basename "${f}")" == "${base}" ]] && continue
    if pngs_identical "${f}" "${dir}/${base}"; then
      echo "drop duplicate ${f} (same as ${base})" >&2
      rm -f "${f}"
    fi
  done
}

# Menu ~0–28px, tab bar ~30–62px on a 1200×800 decorated Studio/Orca frame.
click_tabs_bambu() {
  local disp="$1"
  local dest_dir="$2"
  # Region Next, Get Started, plugin OK / Skip.
  click_xy "${disp}" 600 430 || true
  sleep 0.3
  click_xy "${disp}" 820 590 || true
  sleep 0.8
  click_xy "${disp}" 600 480 || true
  sleep 0.8
  click_xy "${disp}" 780 430 || true
  sleep 0.5
  click_xy "${disp}" 36 48
  sleep 2
  grab_named "${disp}" "${dest_dir}/home.png"
  click_xy "${disp}" 130 48
  sleep 3
  grab_named "${disp}" "${dest_dir}/prepare.png"
  click_xy "${disp}" 250 48
  sleep 2
  grab_named "${disp}" "${dest_dir}/preview.png"
  click_xy "${disp}" 370 48
  sleep 2
  grab_named "${disp}" "${dest_dir}/device.png"
  click_xy "${disp}" 640 48
  sleep 3
  grab_named "${disp}" "${dest_dir}/filament.png"
  drop_duplicate_tabs "${dest_dir}" home.png
}

click_tabs_orca() {
  local disp="$1"
  local dest_dir="$2"
  click_xy "${disp}" 600 480 || true
  sleep 0.8
  click_xy "${disp}" 700 510 || true
  sleep 0.8
  click_xy "${disp}" 130 48
  sleep 3
  grab_named "${disp}" "${dest_dir}/prepare.png"
  click_xy "${disp}" 250 48
  sleep 2
  grab_named "${disp}" "${dest_dir}/preview.png"
  drop_duplicate_tabs "${dest_dir}" prepare.png
}

capture_app() {
  local label="$1"
  local dest_dir="$2"
  local family="$3"
  local env_name="$4"
  shift 4
  local bin=""
  if ! bin="$(find_bin "${env_name}" "$@")"; then
    echo "skip ${label}: binary not found" >&2
    return 0
  fi
  echo "capturing ${label} (${bin})" >&2
  mkdir -p "${dest_dir}"

  local home
  home="$(mktemp -d "/tmp/bambu-upstream-${label}-XXXX")"
  seed_family "${family}" "${home}"

  local disp=":$((80 + RANDOM % 20))"
  Xvfb "${disp}" -screen 0 "${W}x${H}x24" >/tmp/bambu-xvfb-"${label}".log 2>&1 &
  local xvfb_pid=$!
  sleep 0.4
  (
    export DISPLAY="${disp}"
    export HOME="${home}"
    export XDG_CONFIG_HOME="${home}/.config"
    export XDG_CACHE_HOME="${home}/.cache"
    export LIBGL_ALWAYS_SOFTWARE=1
    export GALLIUM_DRIVER=llvmpipe
    export MESA_GL_VERSION_OVERRIDE=3.3
    export GDK_BACKEND=x11
    export GTK_THEME=Adwaita:dark
    export WEBKIT_DISABLE_COMPOSITING_MODE=1
    mkdir -p "${XDG_CONFIG_HOME}" "${XDG_CACHE_HOME}"
    exec "${bin}"
  ) >/tmp/bambu-upstream-"${label}".log 2>&1 &
  local app_pid=$!
  sleep "${WAIT}"
  if ! kill -0 "${app_pid}" 2>/dev/null; then
    echo "skip ${label}: process exited (see /tmp/bambu-upstream-${label}.log)" >&2
    kill "${xvfb_pid}" 2>/dev/null || true
    wait "${xvfb_pid}" 2>/dev/null || true
    rm -rf "${home}"
    return 0
  fi

  if command -v xdotool >/dev/null 2>&1; then
    case "${family}" in
      bambu) click_tabs_bambu "${disp}" "${dest_dir}" ;;
      orca) click_tabs_orca "${disp}" "${dest_dir}" ;;
      prusa)
        grab_named "${disp}" "${dest_dir}/prepare.png"
        ;;
    esac
  else
    case "${family}" in
      bambu) grab_named "${disp}" "${dest_dir}/home.png" ;;
      *) grab_named "${disp}" "${dest_dir}/prepare.png" ;;
    esac
  fi

  kill "${app_pid}" "${xvfb_pid}" 2>/dev/null || true
  wait "${app_pid}" "${xvfb_pid}" 2>/dev/null || true
  rm -rf "${home}"
}

ensure_xvfb
ensure_import
add_nix_bin xdotool || echo "xdotool missing: only the landing tab will be grabbed" >&2
mkdir -p "${OUT}/bambu" "${OUT}/orca" "${OUT}/prusa"

case "${UPSTREAM_CAPTURE:-all}" in
  bambu) capture_app bambu "${OUT}/bambu" bambu BAMBU_STUDIO bambu-studio BambuStudio bambu-studio-bin ;;
  orca) capture_app orca "${OUT}/orca" orca ORCA_SLICER orca-slicer ;;
  prusa) capture_app prusa "${OUT}/prusa" prusa PRUSA_SLICER prusa-slicer prusa-slicer-bin ;;
  all)
    capture_app bambu "${OUT}/bambu" bambu BAMBU_STUDIO bambu-studio BambuStudio bambu-studio-bin
    capture_app orca "${OUT}/orca" orca ORCA_SLICER orca-slicer
    capture_app prusa "${OUT}/prusa" prusa PRUSA_SLICER prusa-slicer prusa-slicer-bin
    ;;
  *)
    echo "UPSTREAM_CAPTURE must be all, bambu, orca, or prusa" >&2
    exit 1
    ;;
esac

echo "upstream captures in ${OUT}"
ls -la "${OUT}"/*/*.png 2>/dev/null || true
