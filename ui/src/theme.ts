// Theme system — each theme overrides the app's color tokens (styles.css :root).
// Palettes are drawn from the tradition of Sanzo Wada's *A Dictionary of Color
// Combinations*: muted, considered, named by Japanese traditional colors. The
// default, "Crucible", is a molten vermilion (Shu) — the brand.
//
// Applied by writing CSS custom properties onto <html> (inline styles beat both
// the light :root and the dark @media block), and persisted to localStorage.

export type ThemeTokens = Partial<Record<string, string>>;
export type Theme = {
  id: string;
  /** English display name */
  name: string;
  /** Japanese color name (kanji · romaji) */
  jp: string;
  dark: boolean;
  /** the accent swatch shown in the picker */
  primary: string;
  tokens: ThemeTokens;
};

export const THEMES: Theme[] = [
  {
    id: "crucible",
    name: "Crucible",
    jp: "朱 · shu",
    dark: false,
    primary: "#db4d2b",
    tokens: {
      "--base": "#ffffff",
      "--canvas": "#faf5f0",
      "--panel": "#f2ebe2",
      "--surface": "#fbf7f2",
      "--surface-bright": "#fffdfb",
      "--highlight": "#fdeee7",
      "--text": "#231f1c",
      "--subtext": "#776f66",
      "--muted": "#a89f95",
      "--primary": "#db4d2b",
      "--primary-subtle": "#fbe7df",
      "--secondary": "#c98a2e",
      "--border": "#e4dbd1",
      "--border-variant": "#ece4da",
      "--dots-muted": "#e8ded2",
      "--dots-strong": "#c7bcab",
    },
  },
  {
    id: "ai",
    name: "Indigo",
    jp: "藍 · ai",
    dark: false,
    primary: "#2f4d80",
    tokens: {
      "--base": "#ffffff",
      "--canvas": "#f4f6f9",
      "--panel": "#e9edf3",
      "--surface": "#f7f9fc",
      "--surface-bright": "#ffffff",
      "--highlight": "#e9f0f9",
      "--text": "#1b2230",
      "--subtext": "#66707f",
      "--muted": "#9aa3b1",
      "--primary": "#2f4d80",
      "--primary-subtle": "#e6ecf6",
      "--secondary": "#3f8fa6",
      "--border": "#d9e0e8",
      "--border-variant": "#e6ebf1",
      "--dots-muted": "#dde3ec",
      "--dots-strong": "#b6bfcd",
    },
  },
  {
    id: "kurenai",
    name: "Safflower",
    jp: "紅 · kurenai",
    dark: false,
    primary: "#b03052",
    tokens: {
      "--base": "#ffffff",
      "--canvas": "#faf5f6",
      "--panel": "#f2e9ec",
      "--surface": "#fbf6f7",
      "--surface-bright": "#fffcfd",
      "--highlight": "#fbe9ef",
      "--text": "#241b1f",
      "--subtext": "#7a6a70",
      "--muted": "#ab9ba1",
      "--primary": "#b03052",
      "--primary-subtle": "#f8e5ec",
      "--secondary": "#c98a2e",
      "--border": "#e6dbe0",
      "--border-variant": "#ede4e8",
      "--dots-muted": "#ecdee4",
      "--dots-strong": "#cab3bd",
    },
  },
  {
    id: "uguisu",
    name: "Nightingale",
    jp: "鶯 · uguisu",
    dark: false,
    primary: "#6f7d3f",
    tokens: {
      "--base": "#ffffff",
      "--canvas": "#f7f8f1",
      "--panel": "#ecefe1",
      "--surface": "#faf9f2",
      "--surface-bright": "#fffefb",
      "--highlight": "#f0f3e2",
      "--text": "#22231b",
      "--subtext": "#6f7264",
      "--muted": "#a3a690",
      "--primary": "#6f7d3f",
      "--primary-subtle": "#edf0dd",
      "--secondary": "#c98a2e",
      "--border": "#e0e2d1",
      "--border-variant": "#e9ebdc",
      "--dots-muted": "#e5e7d4",
      "--dots-strong": "#bfc2a4",
    },
  },
  {
    id: "botan",
    name: "Peony",
    jp: "牡丹 · botan",
    dark: false,
    primary: "#bd3a84",
    tokens: {
      "--base": "#ffffff",
      "--canvas": "#faf5f8",
      "--panel": "#f2e9ef",
      "--surface": "#fbf6f9",
      "--surface-bright": "#fffcfe",
      "--highlight": "#fbe8f2",
      "--text": "#241c22",
      "--subtext": "#7a6c74",
      "--muted": "#ab9ca5",
      "--primary": "#bd3a84",
      "--primary-subtle": "#f8e5f0",
      "--secondary": "#8f6fd6",
      "--border": "#e7dbe3",
      "--border-variant": "#eee4ea",
      "--dots-muted": "#eddee7",
      "--dots-strong": "#ccb3c2",
    },
  },
  {
    id: "sumi",
    name: "Ink",
    jp: "墨 · sumi",
    dark: true,
    primary: "#e0813e",
    tokens: {
      "--base": "#15130f",
      "--canvas": "#1a1713",
      "--panel": "#211d18",
      "--surface": "#231f19",
      "--surface-bright": "#2b261f",
      "--highlight": "#2f281f",
      "--text": "#ede7dd",
      "--subtext": "#a79d8e",
      "--muted": "#756d5f",
      "--primary": "#e0813e",
      "--primary-subtle": "#3a2c1c",
      "--secondary": "#c9a24e",
      "--border": "#332e26",
      "--border-variant": "#3d372d",
      "--dots-muted": "#2c2820",
      "--dots-strong": "#4a4437",
    },
  },
];

const STORAGE_KEY = "crux:theme";
export const DEFAULT_THEME = "crucible";

export function getTheme(id: string): Theme {
  return THEMES.find((t) => t.id === id) ?? THEMES[0];
}

/** Write a theme's tokens onto <html> and mark dark/light. */
export function applyTheme(id: string) {
  const theme = getTheme(id);
  const root = document.documentElement;
  for (const [k, v] of Object.entries(theme.tokens)) {
    if (v != null) root.style.setProperty(k, v);
  }
  root.setAttribute("data-theme", theme.id);
  root.setAttribute("data-theme-mode", theme.dark ? "dark" : "light");
  root.style.colorScheme = theme.dark ? "dark" : "light";
}

export function loadThemeId(): string {
  try {
    return localStorage.getItem(STORAGE_KEY) || DEFAULT_THEME;
  } catch {
    return DEFAULT_THEME;
  }
}

export function saveThemeId(id: string) {
  try {
    localStorage.setItem(STORAGE_KEY, id);
  } catch {
    /* ignore */
  }
  applyTheme(id);
}

/** Call once at startup, before first paint, to avoid a flash. */
export function initTheme() {
  applyTheme(loadThemeId());
}
