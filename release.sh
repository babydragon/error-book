#!/usr/bin/env bash
set -euo pipefail

APP_NAME="error-book"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIST_DIR="$ROOT_DIR/release-artifacts"
RISCV_TARGET="riscv64gc-unknown-linux-gnu"
HOST_ARCH="x86_64"
RISC_ARCH="riscv64gc"

VERSION="$(python - <<'PY'
from pathlib import Path
import re
text = Path('Cargo.toml').read_text(encoding='utf-8')
m = re.search(r'^version\s*=\s*"([^"]+)"', text, re.M)
print(m.group(1) if m else '0.1.0')
PY
)"

HOST_BIN="$ROOT_DIR/target/release/$APP_NAME"
RISCV_BIN="$ROOT_DIR/target/$RISCV_TARGET/release/$APP_NAME"

FONT_FILE="$(compgen -G "$ROOT_DIR/fonts/NotoSansSC-Regular.*" | head -n 1 || true)"
if [[ -z "$FONT_FILE" ]]; then
  FONT_FILE="$(compgen -G "$ROOT_DIR/fonts/NotoSansCJK-Regular.*" | head -n 1 || true)"
fi
OFL_FILE="$(compgen -G "$ROOT_DIR/fonts/OFL*" | head -n 1 || true)"

if [[ -z "$FONT_FILE" ]]; then
  echo "❌ 未找到 Noto 字体文件，请确认 fonts/ 目录下存在 NotoSansSC-Regular.ttf 或 NotoSansCJK-Regular.*" >&2
  exit 1
fi

if [[ -z "$OFL_FILE" ]]; then
  echo "❌ 未找到 OFL 许可证文件，请确认 fonts/ 目录下存在 OFL.txt / OFL.md 等文件" >&2
  exit 1
fi

echo "==> 构建本机 release"
cargo build --release

echo "==> 构建 RISC-V release"
cargo zigbuild --release --target "$RISCV_TARGET"

if [[ ! -f "$HOST_BIN" ]]; then
  echo "❌ 未找到本机构建产物: $HOST_BIN" >&2
  exit 1
fi

if [[ ! -f "$RISCV_BIN" ]]; then
  echo "❌ 未找到 RISC-V 构建产物: $RISCV_BIN" >&2
  exit 1
fi

rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"

package_target() {
  local arch="$1"
  local bin_path="$2"
  local out_dir="$DIST_DIR/${APP_NAME}-v${VERSION}-${arch}"

  mkdir -p "$out_dir/fonts"
  mkdir -p "$out_dir/skills/error-intake"
  mkdir -p "$out_dir/skills/summary-practice-coach"

  cp "$bin_path" "$out_dir/$APP_NAME"
  chmod +x "$out_dir/$APP_NAME"

  if [[ -f "$ROOT_DIR/config.example.toml" ]]; then
    cp "$ROOT_DIR/config.example.toml" "$out_dir/config.example.toml"
  fi
  if [[ -f "$ROOT_DIR/README.md" ]]; then
    cp "$ROOT_DIR/README.md" "$out_dir/README.md"
  fi

  cp "$FONT_FILE" "$out_dir/fonts/"
  cp "$OFL_FILE" "$out_dir/fonts/"

  if [[ -f "$ROOT_DIR/skills/error-intake/SKILL.md" ]]; then
    cp "$ROOT_DIR/skills/error-intake/SKILL.md" "$out_dir/skills/error-intake/SKILL.md"
  fi
  if [[ -f "$ROOT_DIR/skills/summary-practice-coach/SKILL.md" ]]; then
    cp "$ROOT_DIR/skills/summary-practice-coach/SKILL.md" "$out_dir/skills/summary-practice-coach/SKILL.md"
  fi

  tar -C "$DIST_DIR" -czf "$DIST_DIR/${APP_NAME}-v${VERSION}-${arch}.tar.gz" "${APP_NAME}-v${VERSION}-${arch}"
  echo "✅ 已生成: $out_dir"
  echo "✅ 已打包: $DIST_DIR/${APP_NAME}-v${VERSION}-${arch}.tar.gz"
}

package_target "$HOST_ARCH" "$HOST_BIN"
package_target "$RISC_ARCH" "$RISCV_BIN"

echo
echo "发布产物目录: $DIST_DIR"
ls -1 "$DIST_DIR"
