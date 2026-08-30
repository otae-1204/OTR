"""同步 OTR 的透明图标源文件。

当前图标是插画资产，不再使用旧的几何图形脚本绘制。同步源图后，
可运行 ``npx tauri icon scripts/icon-source.png -o src-tauri/icons``
重新生成各平台图标。
"""

from pathlib import Path
import shutil


ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "scripts" / "otr-icon-transparent-v1.png"
TARGET = ROOT / "scripts" / "icon-source.png"


def main() -> None:
    if not SOURCE.is_file():
        raise FileNotFoundError(f"icon source not found: {SOURCE}")
    shutil.copyfile(SOURCE, TARGET)
    print(f"synced: {TARGET}")


if __name__ == "__main__":
    main()
