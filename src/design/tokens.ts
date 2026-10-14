// Design tokens — single source of truth. Mirrors tokens.css for non-styled consumers (Pixi).
// Any change here must also update tokens.css.

export const colors = {
  // v0.5 re-theme (light values; mirrors tokens.css): cool slate neutrals,
  // navy ink, teal brand.
  cream: {
    50: 0xf7f9fb,
    100: 0xeef2f6,
    200: 0xe1e8ef,
    300: 0xcbd6e1
  },
  paper: {
    100: 0xfbfcfd,
    200: 0xedf1f5
  },
  ink: {
    900: 0x0f1c2e,
    700: 0x2c3e55,
    500: 0x50627a,
    300: 0x8a9baf,
    100: 0xccd6e1
  },
  brand: 0x1b9890,
  // v0.3.4 recalibration: same hues, professional saturation (mirrors tokens.css)
  accent: {
    coral: 0xda5f66,
    coralLight: 0xf7d6d8,
    mint: 0x41aa84,
    mintLight: 0xcfebdf,
    sky: 0x3b93d6,
    skyLight: 0xd0e5f7,
    lemon: 0xe1ae2e,
    lemonLight: 0xf5e7be,
    lilac: 0x8478e0,
    lilacLight: 0xdedaf7,
    peach: 0xde8f6f,
    peachLight: 0xf5dccc
  },
  status: {
    idle: 0x9aa7b8,
    thinking: 0x3b93d6,
    working: 0xe1ae2e,
    blocked: 0xda5f66,
    success: 0x41aa84,
    ghost: 0xd3dce6
  },
  world: {
    grassLight: 0xd4eab0,
    grassDark: 0xb5d589,
    woodLight: 0xe5c896,
    woodDark: 0xc9a66b,
    path: 0xe8d8b0,
    wall: 0x8b6f47
  }
} as const;

export const space = {
  0: 0, 1: 4, 2: 8, 3: 12, 4: 16, 5: 24, 6: 32, 7: 48, 8: 64
} as const;

export const type = {
  display: '"Press Start 2P", "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", "Noto Sans CJK SC", "Geeza Pro", "Noto Naskh Arabic", "Segoe UI Historic", monospace',
  ui: '"Inter", -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", "Noto Sans CJK SC", "Geeza Pro", "Noto Naskh Arabic", "Segoe UI Historic", sans-serif',
  mono: '"JetBrains Mono", ui-monospace, "SF Mono", Menlo, "PingFang SC", "Microsoft YaHei", "Noto Sans Mono CJK SC", "Noto Sans CJK SC", "Geeza Pro", "Noto Naskh Arabic", "Segoe UI Historic", monospace'
} as const;

export const tileSize = 32; // px — the world is built from 32×32 tiles

export type AccentColorName =
  | 'coral' | 'mint' | 'sky' | 'lemon' | 'lilac' | 'peach';

export const accentByName: Record<AccentColorName, number> = {
  coral: colors.accent.coral,
  mint:  colors.accent.mint,
  sky:   colors.accent.sky,
  lemon: colors.accent.lemon,
  lilac: colors.accent.lilac,
  peach: colors.accent.peach
};

export const accentLightByName: Record<AccentColorName, number> = {
  coral: colors.accent.coralLight,
  mint:  colors.accent.mintLight,
  sky:   colors.accent.skyLight,
  lemon: colors.accent.lemonLight,
  lilac: colors.accent.lilacLight,
  peach: colors.accent.peachLight
};

// Convert 0xRRGGBB to "#RRGGBB"
export function hex(c: number): string {
  return '#' + c.toString(16).padStart(6, '0').toUpperCase();
}
