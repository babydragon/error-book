#!/usr/bin/env bash
set -euo pipefail

APP_NAME="error-book"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARGET_DIR="$ROOT_DIR/target"
PACKAGE_ROOT="$TARGET_DIR/release-artifacts"
RISCV_TARGET="riscv64gc-unknown-linux-gnu"

VERSION="$(python - <<'PY'
from pathlib import Path
import re
text = Path('Cargo.toml').read_text(encoding='utf-8')
m = re.search(r'^version\s*=\s*"([^"]+)"', text, re.M)
print(m.group(1) if m else '0.1.0')
PY
)"

usage() {
  cat <<'EOF'
用法：
  ./release.sh [架构...]

支持的架构：
  x86_64      -> 本机 x86_64 Linux release 构建
  riscv64gc   -> riscv64gc-unknown-linux-gnu，使用 cargo zigbuild

默认行为：
  - 如果不传参数：构建本机默认架构 + riscv64gc
  - 所有产物输出到 target/release-artifacts/

示例：
  ./release.sh
  ./release.sh x86_64
  ./release.sh riscv64gc
  ./release.sh x86_64 riscv64gc
EOF
}

detect_host_arch() {
  local machine
  machine="$(uname -m)"
  case "$machine" in
    x86_64|amd64) echo "x86_64" ;;
    riscv64|riscv64gc) echo "riscv64gc" ;;
    *)
      echo "不支持自动识别的本机架构: $machine，请显式传参，例如 ./release.sh x86_64 riscv64gc" >&2
      exit 1
      ;;
  esac
}

ensure_font_assets() {
  local fonts_dir="$ROOT_DIR/fonts"

  FONT_FILE="$(compgen -G "$fonts_dir/NotoSansSC-Regular.*" | head -n 1 || true)"
  if [[ -z "$FONT_FILE" ]]; then
    FONT_FILE="$(compgen -G "$fonts_dir/NotoSansCJKsc-Regular.*" | head -n 1 || true)"
  fi
  if [[ -z "$FONT_FILE" ]]; then
    FONT_FILE="$(compgen -G "$fonts_dir/NotoSansCJK-Regular.*" | head -n 1 || true)"
  fi

  OFL_FILE="$(compgen -G "$fonts_dir/OFL*" | head -n 1 || true)"

  if [[ -z "$FONT_FILE" ]]; then
    cat >&2 <<'EOF'
❌ 未找到 Noto Sans SC 字体文件。

请先从 Google/Noto 官方渠道下载并放到项目 fonts/ 目录，例如：
- NotoSansSC-Regular.ttf
- 或 NotoSansCJKsc-Regular.otf / NotoSansCJK-Regular.otf

推荐搜索：Google Noto Sans SC download
EOF
    exit 1
  fi

  if [[ -z "$OFL_FILE" ]]; then
    cat >&2 <<'EOF'
❌ 未找到字体授权文件。

请将 Noto 字体附带的 OFL 许可证文件一并放到 fonts/ 目录，例如：
- OFL.txt
- OFL.md
EOF
    exit 1
  fi
}

ensure_command() {
  local cmd="$1"
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "❌ 缺少命令: $cmd" >&2
    exit 1
  fi
}

normalize_arch() {
  case "$1" in
    x86_64|amd64) echo "x86_64" ;;
    riscv64gc|riscv64) echo "riscv64gc" ;;
    *)
      echo "❌ 不支持的架构: $1，可选: x86_64, riscv64gc" >&2
      exit 1
      ;;
  esac
}

unique_arches() {
  python - "$@" <<'PY'
import sys
seen = set()
for item in sys.argv[1:]:
    if item not in seen:
        seen.add(item)
        print(item)
PY
}

build_target() {
  local arch="$1"
  case "$arch" in
    x86_64)
      echo "==> 构建 x86_64 release"
      cargo build --release
      ;;
    riscv64gc)
      ensure_command cargo-zigbuild
      echo "==> 构建 riscv64gc release (cargo zigbuild)"
      cargo zigbuild --release --target "$RISCV_TARGET"
      ;;
  esac
}

binary_path_for() {
  local arch="$1"
  case "$arch" in
    x86_64) echo "$ROOT_DIR/target/release/$APP_NAME" ;;
    riscv64gc) echo "$ROOT_DIR/target/$RISCV_TARGET/release/$APP_NAME" ;;
  esac
}

package_dir_for() {
  local arch="$1"
  echo "$PACKAGE_ROOT/${APP_NAME}-v${VERSION}-${arch}"
}

archive_path_for() {
  local arch="$1"
  echo "$PACKAGE_ROOT/${APP_NAME}-v${VERSION}-${arch}.tar.zst"
}

checksum_path_for() {
  local archive_path="$1"
  echo "${archive_path}.sha256"
}

package_target() {
  local arch="$1"
  local bin_path
  local out_dir
  local archive_path
  local checksum_path

  bin_path="$(binary_path_for "$arch")"
  out_dir="$(package_dir_for "$arch")"
  archive_path="$(archive_path_for "$arch")"
  checksum_path="$(checksum_path_for "$archive_path")"

  if [[ ! -f "$bin_path" ]]; then
    echo "❌ 未找到构建产物: $bin_path" >&2
    exit 1
  fi

  rm -rf "$out_dir"
  mkdir -p "$out_dir/fonts"
  mkdir -p "$out_dir/skills/error-intake"
  mkdir -p "$out_dir/skills/summary-practice-coach"

  cp "$bin_path" "$out_dir/$APP_NAME"
  chmod +x "$out_dir/$APP_NAME"

  [[ -f "$ROOT_DIR/config.example.toml" ]] && cp "$ROOT_DIR/config.example.toml" "$out_dir/config.example.toml"
  [[ -f "$ROOT_DIR/README.md" ]] && cp "$ROOT_DIR/README.md" "$out_dir/README.md"

  cp "$FONT_FILE" "$out_dir/fonts/"
  cp "$OFL_FILE" "$out_dir/fonts/"

  [[ -f "$ROOT_DIR/skills/error-intake/SKILL.md" ]] && cp "$ROOT_DIR/skills/error-intake/SKILL.md" "$out_dir/skills/error-intake/SKILL.md"
  [[ -f "$ROOT_DIR/skills/summary-practice-coach/SKILL.md" ]] && cp "$ROOT_DIR/skills/summary-practice-coach/SKILL.md" "$out_dir/skills/summary-practice-coach/SKILL.md"

  rm -f "$archive_path" "$checksum_path"
  tar -C "$PACKAGE_ROOT" --zstd -cf "$archive_path" "$(basename "$out_dir")"

  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$(basename "$archive_path")" > "$checksum_path"
  elif command -v shasum >/dev/null 2>&1; then
    (cd "$PACKAGE_ROOT" && shasum -a 256 "$(basename "$archive_path")" > "$(basename "$checksum_path")")
  else
    echo "⚠️ 未找到 sha256sum/shasum，跳过校验文件生成" >&2
  fi

  echo "✅ 已生成目录: $out_dir"
  echo "✅ 已生成压缩包: $archive_path"
  [[ -f "$checksum_path" ]] && echo "✅ 已生成校验文件: $checksum_path"
}

main() {
  if [[ ${1:-} == "-h" || ${1:-} == "--help" ]]; then
    usage
    exit 0
  fi

  ensure_command cargo
  ensure_command tar

  cd "$ROOT_DIR"
  ensure_font_assets

  local host_arch
  host_arch="$(detect_host_arch)"

  local raw_arches=()
  if [[ $# -eq 0 ]]; then
    raw_arches=("$host_arch")
    if [[ "$host_arch" != "riscv64gc" ]]; then
      raw_arches+=("riscv64gc")
    fi
  else
    local arg
    for arg in "$@"; do
      raw_arches+=("$(normalize_arch "$arg")")
    done
  fi

  mapfile -t TARGET_ARCHES < <(unique_arches "${raw_arches[@]}")

  mkdir -p "$PACKAGE_ROOT"

  local arch
  for arch in "${TARGET_ARCHES[@]}"; do
    build_target "$arch"
  done

  for arch in "${TARGET_ARCHES[@]}"; do
    package_target "$arch"
  done

  echo
  echo "发布产物目录: $PACKAGE_ROOT"
  ls -1 "$PACKAGE_ROOT"
  echo
  echo "说明：上传 GitHub Release 时，建议连同 .sha256 文件一起上传，便于校验下载完整性。"
}

main "$@"
