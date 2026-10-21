#!/usr/bin/env node
/**
 * 生成应用图标源图（1024×1024 PNG），供 `tauri icon` 派生出各尺寸图标。
 *
 * 设计：圆角方形苹果蓝底（#007AFF）+ 白色双向箭头（呼应「本地代理转发」）。
 * 不依赖任何图形库：手写 SDF 光栅化 + 3×3 超采样抗锯齿 + 手写 PNG 编码。
 */

import { deflateSync } from 'node:zlib';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const SIZE = 1024;
const OUT = join(HERE, '..', 'desktop-tauri', 'src-tauri', 'icons', 'source.png');

// ─── PNG 编码 ──────────────────────────────────────────────

const CRC_TABLE = (() => {
  const table = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c;
  }
  return table;
})();

function crc32(buffer) {
  let c = 0xffffffff;
  for (const byte of buffer) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'latin1'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, crc]);
}

function encodePng(width, height, rgba) {
  const stride = width * 4;
  const raw = Buffer.alloc((stride + 1) * height);
  for (let y = 0; y < height; y++) {
    raw[y * (stride + 1)] = 0; // filter: none
    rgba.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8;   // bit depth
  ihdr[9] = 6;   // RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

// ─── 形状：有符号距离场（SDF），返回 <0 表示在形状内 ──────────

/** 点到线段的最短距离（胶囊体 SDF 的基础） */
function segmentDistance(px, py, ax, ay, bx, by) {
  const vx = bx - ax;
  const vy = by - ay;
  const wx = px - ax;
  const wy = py - ay;
  const len2 = vx * vx + vy * vy;
  const t = len2 === 0 ? 0 : Math.max(0, Math.min(1, (wx * vx + wy * vy) / len2));
  const dx = px - (ax + t * vx);
  const dy = py - (ay + t * vy);
  return Math.hypot(dx, dy);
}

function sdRoundedRect(px, py, halfW, halfH, radius) {
  const dx = Math.abs(px) - (halfW - radius);
  const dy = Math.abs(py) - (halfH - radius);
  const outside = Math.hypot(Math.max(dx, 0), Math.max(dy, 0));
  return outside + Math.min(Math.max(dx, dy), 0) - radius;
}

function sdCapsule(px, py, ax, ay, bx, by, thickness) {
  return segmentDistance(px, py, ax, ay, bx, by) - thickness;
}

/**
 * 双向箭头（⇄）：上箭头朝右、下箭头朝左。
 * 坐标以 [0,1] 为画布（shapeAlpha 里再减去 0.5 归一化）。
 * 斜线起点收进横杆末端，让横杆与箭头衔接成一个箭头形状而不是一坨。
 */
const ARROW_THICKNESS = 0.046;
const TOP_Y = 0.385;
const BOT_Y = 0.615;
const TIP_X = 0.782;
const TAIL_X = 0.218;
const HEAD_HALF = 0.105;   // 箭头张开的半高
const HEAD_BACK = 0.118;   // 从尖端往回收的水平距离
const STROKES = [
  // 上：横杆到箭头根部，尖端在右
  [TAIL_X, TOP_Y, TIP_X - HEAD_BACK, TOP_Y],
  [TIP_X, TOP_Y, TIP_X - HEAD_BACK, TOP_Y - HEAD_HALF],
  [TIP_X, TOP_Y, TIP_X - HEAD_BACK, TOP_Y + HEAD_HALF],
  // 下：横杆到箭头根部，尖端在左
  [TIP_X, BOT_Y, TAIL_X + HEAD_BACK, BOT_Y],
  [TAIL_X, BOT_Y, TAIL_X + HEAD_BACK, BOT_Y - HEAD_HALF],
  [TAIL_X, BOT_Y, TAIL_X + HEAD_BACK, BOT_Y + HEAD_HALF],
];

/** 底为圆角方形，前景为箭头；坐标已归一化到 [-0.5, 0.5] */
function shapeAlpha(nx, ny) {
  const insideRect = sdRoundedRect(nx, ny, 0.5, 0.5, 0.225);
  let arrow = Infinity;
  for (const [ax, ay, bx, by] of STROKES) {
    arrow = Math.min(arrow, sdCapsule(nx, ny, ax - 0.5, ay - 0.5, bx - 0.5, by - 0.5, ARROW_THICKNESS));
  }
  return { rect: insideRect, arrow };
}

// ─── 光栅化（3×3 超采样）────────────────────────────────────

const SS = 3;

/** 线性插值：目前只用于把纯白箭头按覆盖率混到纯色底上 */
function lerp(a, b, t) {
  return a + (b - a) * t;
}

/**
 * 把一块画布光栅化成 RGBA 像素。
 *
 * `shape` 收归一化坐标（[-0.5, 0.5]），返回 `{ rect, arrow }` 两个 SDF 值。
 * 分成「底」与「箭头」两层是为了让两者各自带抗锯齿覆盖率 —— 箭头压在底上，
 * 边缘不能直接取整。
 */
function rasterize(size, shape) {
  const pixels = Buffer.alloc(size * size * 4);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let rectHits = 0;
      let arrowHits = 0;
      for (let sy = 0; sy < SS; sy++) {
        for (let sx = 0; sx < SS; sx++) {
          const px = (x + (sx + 0.5) / SS) / size - 0.5;
          const py = (y + (sy + 0.5) / SS) / size - 0.5;
          const { rect, arrow } = shape(px, py);
          if (rect <= 0) rectHits++;
          if (arrow <= 0) arrowHits++;
        }
      }
      const total = SS * SS;
      const rectA = rectHits / total;
      const arrowA = arrowHits / total;
      if (rectA === 0) continue;

      // 底色改为纯苹果蓝 #007AFF（与 iOS/macOS 系统蓝同族的观感），不再做对角渐变，
      // 这样图标在浅色/深色壁纸和 Dock 里都保持同一个可辨识的品牌蓝。
      let r = 0x00;
      let g = 0x7a;
      let b = 0xff;
      // 箭头为纯白，按覆盖率与底色混合
      if (arrowA > 0) {
        r = lerp(r, 255, arrowA);
        g = lerp(g, 255, arrowA);
        b = lerp(b, 255, arrowA);
      }
      const i = (y * size + x) * 4;
      pixels[i] = Math.round(r);
      pixels[i + 1] = Math.round(g);
      pixels[i + 2] = Math.round(b);
      pixels[i + 3] = Math.round(rectA * 255);
    }
  }
  return pixels;
}

const pixels = rasterize(SIZE, shapeAlpha);

mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, encodePng(SIZE, SIZE, pixels));
console.log(`[icon] 已生成 ${OUT}`);

// ─── PWA maskable 图标 ──────────────────────────────────────
//
// 「maskable」是 Android 自适应图标用的那一档：系统会**按自己的形状裁剪**
// （圆形、方形、水滴…），并可能把图形放大到 1.33 倍。规范要求关键内容落在
// 中心 80% 直径的圆内，否则会被裁掉。
//
// 所以这一版与 source.png 有两处必须不同，不能直接复用同一张图：
//   ① **满铺、不透明、无圆角** —— 带透明圆角的图被裁成圆形后，四角会露出
//      底色（黑边），且规范明确要求 maskable 图标不能有透明像素；
//   ② **箭头缩到 60%** —— 留出被裁剪的余量，放大 1.33 倍后仍完整。
//
// 输出直接落到 `ui/assets/`（手机端静态资源目录），不进 tauri icons ——
// 那一套是桌面端安装包用的，与 PWA 无关。
const MASKABLE_OUT = join(HERE, '..', 'desktop-tauri', 'ui', 'assets', 'app-icon-maskable-512.png');
const MASKABLE_SIZE = 512;
/** 箭头缩放：0.6 时箭头横向跨度约 0.34，落在 80% 安全圆内绰绰有余 */
const MASKABLE_SCALE = 0.6;

const maskablePixels = rasterize(MASKABLE_SIZE, (nx, ny) => {
  // 底：满铺整块画布（SDF 恒 <0），于是没有透明像素、也没有圆角
  const rect = -1;
  // 箭头：坐标放大 = 图形缩小，居中不变
  const { arrow } = shapeAlpha(nx / MASKABLE_SCALE, ny / MASKABLE_SCALE);
  return { rect, arrow };
});

writeFileSync(MASKABLE_OUT, encodePng(MASKABLE_SIZE, MASKABLE_SIZE, maskablePixels));
console.log(`[icon] 已生成 ${MASKABLE_OUT}`);
