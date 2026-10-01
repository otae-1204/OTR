import { useTheme } from "../theme/ThemeProvider";
import darkArt from "../theme/art/blue-whale-bg-dark.jpg";
import lightArt from "../theme/art/blue-whale-bg-light.jpg";

/** 内置主题「蓝色大肥鱼」的整窗背景。主题 JSON 不能嵌图片,所以按主题 id 铺在所有界面后面。 */
const ART: Record<string, { dark: string; light: string }> = {
  "blue-whale": { dark: darkArt, light: lightArt },
};

export function ThemeMascot() {
  const { theme } = useTheme();
  const pair = ART[theme.id];
  if (!pair) return null;
  return (
    <div aria-hidden className="pointer-events-none fixed inset-0 z-0">
      <img
        src={theme.mode === "light" ? pair.light : pair.dark}
        alt=""
        draggable={false}
        className="h-full w-full select-none object-cover object-[70%_center]"
      />
    </div>
  );
}
