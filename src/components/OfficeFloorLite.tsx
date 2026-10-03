import { useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import type { Delivery } from '../api';
import { useApp } from '../store';
import mapUrl from '../assets/maps/office.tmj?url';
import tilesetUrl from '../assets/tilesets/office-tileset.png?url';
import a5Url from '../assets/tilesets/a5-office-floors-walls.png?url';
import interiorsUrl from '../assets/tilesets/interiors.png?url';

/**
 * OfficeFloorLite — a canvas-2d rendering of the same Tiled office map +
 * tilesets the Pixi floor uses (assets copied verbatim, see ATTRIBUTION.md).
 * The full animated floor (walk paths, wander behaviors, thought bubbles,
 * 85KB OfficeFloor.tsx) is a later phase; this keeps the product's visual
 * identity live on real hive data now:
 *   - agents sit at the map's named spawn points (god → desk-ceo)
 *   - status drives the avatar's color/pulse (hook-derived)
 *   - hive-deliver events fly envelopes between desks
 */

const TILE = 16;

interface TmjLayer {
  name: string;
  type: string;
  width: number;
  height: number;
  data?: number[];
  objects?: { name: string; x: number; y: number }[];
}
interface Tmj {
  width: number;
  height: number;
  tilewidth: number;
  tileheight: number;
  tilesets: { firstgid: number; source?: string; image?: string; tilecount?: number }[];
  layers: TmjLayer[];
}

const STATUS_COLOR: Record<string, string> = {
  working: '#58a6ff',
  idle: '#4caf50',
  waiting: '#ff9f43',
  compacting: '#b983ff',
  starting: '#f4d35e',
  stopped: '#666'
};

interface Envelope {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
  t0: number;
  dur: number;
}

/** Spawn-point assignment: god parks on desk-ceo, workers take pc-N then the
 *  named desks, overflow stands at the cafeteria seats. Deterministic per
 *  roster order so seats don't shuffle on refresh. */
function seatFor(index: number, isGod: boolean, spawns: Map<string, { x: number; y: number }>): { x: number; y: number } {
  const pick = (name: string, fb: { x: number; y: number }) => spawns.get(name) ?? fb;
  if (isGod) return pick('desk-ceo', { x: 48, y: 64 });
  if (index <= 6) return pick(`pc-${index}`, { x: 32 + index * 64, y: 208 });
  const deskNames = [
    'desk-team-lead',
    'desk-backend-engineer',
    'desk-product-manager',
    'desk-data-engineer',
    'desk-project-manager',
    'desk-market-researcher',
    'desk-agent-organizer',
    'desk-chief-architect',
    'desk-ui-ux-expert'
  ];
  const d = deskNames[index - 7];
  if (d) return pick(d, { x: 32, y: 288 });
  return pick(`cafe-seat-${((index - 16) % 4) + 1}`, { x: 400 + (index % 4) * 32, y: 240 });
}

export function OfficeFloorLite() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const agents = useApp((s) => s.agents);
  const agentsRef = useRef(agents);
  agentsRef.current = agents;
  const envelopes = useRef<Envelope[]>([]);
  const spawnsRef = useRef<Map<string, { x: number; y: number }>>(new Map());
  const seatsRef = useRef<Map<string, { x: number; y: number }>>(new Map());

  // Envelope flights between desks on hive deliveries.
  useEffect(() => {
    const un = listen<Delivery>('hive-deliver', (e) => {
      const d = e.payload;
      const from = seatsRef.current.get(d.from) ?? { x: 272, y: 176 };
      const to = seatsRef.current.get(d.to) ?? { x: 272, y: 176 };
      envelopes.current.push({ x0: from.x, y0: from.y - 10, x1: to.x, y1: to.y - 10, t0: performance.now(), dur: 900 });
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    let dead = false;
    let raf = 0;
    const canvas = canvasRef.current!;

    (async () => {
      const [map, imgMain, imgA5, imgInt] = await Promise.all([
        fetch(mapUrl).then((r) => r.json() as Promise<Tmj>),
        loadImg(tilesetUrl),
        loadImg(a5Url),
        loadImg(interiorsUrl)
      ]);
      if (dead) return;

      const sets: { first: number; img: HTMLImageElement }[] = [
        { first: 1, img: imgMain },
        { first: 513, img: imgA5 },
        { first: 1025, img: imgInt }
      ];
      const setFor = (gid: number) => {
        for (let i = sets.length - 1; i >= 0; i--) {
          if (gid >= sets[i].first) return sets[i];
        }
        return null;
      };

      // Pre-render static layers into one offscreen canvas per z-band:
      // below agents (floor+walls+furniture-below) and above (furniture-above).
      const W = map.width * TILE;
      const H = map.height * TILE;
      const mk = () => {
        const c = document.createElement('canvas');
        c.width = W;
        c.height = H;
        return c.getContext('2d')!;
      };
      const below = mk();
      const above = mk();
      below.imageSmoothingEnabled = false;
      above.imageSmoothingEnabled = false;

      const drawLayer = (ctx: CanvasRenderingContext2D, layer: TmjLayer) => {
        if (!layer.data) return;
        layer.data.forEach((gid, i) => {
          if (gid === 0) return;
          // Tiled flip flags live in the high bits — mask them off.
          const clean = gid & 0x1fffffff;
          const set = setFor(clean);
          if (!set) return;
          const idx = clean - set.first;
          const cols = set.img.width / TILE;
          const sx = (idx % cols) * TILE;
          const sy = Math.floor(idx / cols) * TILE;
          ctx.drawImage(set.img, sx, sy, TILE, TILE, (i % map.width) * TILE, Math.floor(i / map.width) * TILE, TILE, TILE);
        });
      };
      for (const l of map.layers) {
        if (l.name === 'floor' || l.name === 'walls' || l.name === 'furniture-below') drawLayer(below, l);
        if (l.name === 'furniture-above') drawLayer(above, l);
        if (l.type === 'objectgroup' && l.name === 'spawn-points') {
          for (const o of l.objects ?? []) spawnsRef.current.set(o.name, { x: o.x, y: o.y });
        }
      }

      const ctx = canvas.getContext('2d')!;
      const frame = (now: number) => {
        if (dead) return;
        const zoom = Math.max(1, Math.floor((canvas.width / W) * 2) / 2);
        ctx.imageSmoothingEnabled = false;
        ctx.fillStyle = '#141416';
        ctx.fillRect(0, 0, canvas.width, canvas.height);
        ctx.setTransform(zoom, 0, 0, zoom, Math.floor((canvas.width - W * zoom) / 2), Math.floor((canvas.height - H * zoom) / 2));
        ctx.drawImage(below.canvas, 0, 0);

        // Assign seats deterministically and draw agents.
        const roster = agentsRef.current;
        let workerIdx = 0;
        const seatMap = new Map<string, { x: number; y: number }>();
        for (const a of roster) {
          const idx = a.is_god ? -1 : ++workerIdx;
          const seat = seatFor(idx < 0 ? 0 : idx, a.is_god, spawnsRef.current);
          seatMap.set(a.id, seat);
          const pulse = a.status === 'working' ? 1 + Math.sin(now / 300) * 0.15 : 1;
          const color = STATUS_COLOR[a.status] ?? '#8a8794';
          const x = seat.x;
          const y = seat.y;
          // status glow under the chair
          ctx.fillStyle = color + '44';
          ctx.beginPath();
          ctx.ellipse(x, y + 4, 9 * pulse, 4 * pulse, 0, 0, Math.PI * 2);
          ctx.fill();
          // pixel person: head + body (procedural stand-in for the sprite
          // generator until the real Character pipeline lands)
          ctx.fillStyle = '#e8b88a';
          ctx.fillRect(x - 3, y - 14, 6, 5); // head
          ctx.fillStyle = color;
          ctx.fillRect(x - 4, y - 9, 8, 9); // shirt
          ctx.fillStyle = '#2c2c33';
          ctx.fillRect(x - 4, y, 3, 4); // legs
          ctx.fillRect(x + 1, y, 3, 4);
          // name
          ctx.fillStyle = 'rgba(232,230,225,0.9)';
          ctx.font = '5px monospace';
          ctx.textAlign = 'center';
          ctx.fillText(a.name.slice(0, 10), x, y + 10);
          if (a.is_god) {
            ctx.fillStyle = '#f4d35e';
            ctx.fillText('★', x, y - 17);
          }
        }
        seatsRef.current = seatMap;

        // Envelopes in flight.
        envelopes.current = envelopes.current.filter((e) => now - e.t0 < e.dur);
        for (const e of envelopes.current) {
          const t = (now - e.t0) / e.dur;
          const arc = Math.sin(t * Math.PI) * 14;
          const x = e.x0 + (e.x1 - e.x0) * t;
          const y = e.y0 + (e.y1 - e.y0) * t - arc;
          ctx.fillStyle = '#f4d35e';
          ctx.fillRect(x - 3, y - 2, 6, 4);
          ctx.strokeStyle = '#7a6a2a';
          ctx.strokeRect(x - 3.5, y - 2.5, 7, 5);
        }

        ctx.drawImage(above.canvas, 0, 0);
        raf = requestAnimationFrame(frame);
      };
      raf = requestAnimationFrame(frame);
    })();

    const fit = () => {
      const w = wrapRef.current;
      if (w && canvas) {
        canvas.width = w.clientWidth;
        canvas.height = w.clientHeight;
      }
    };
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(wrapRef.current!);
    return () => {
      dead = true;
      cancelAnimationFrame(raf);
      ro.disconnect();
    };
  }, []);

  return (
    <div ref={wrapRef} className="floor-wrap">
      <canvas ref={canvasRef} />
    </div>
  );
}

function loadImg(src: string): Promise<HTMLImageElement> {
  return new Promise((res, rej) => {
    const i = new Image();
    i.onload = () => res(i);
    i.onerror = rej;
    i.src = src;
  });
}
