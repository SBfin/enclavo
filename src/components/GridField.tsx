"use client";

import { useEffect, useRef } from "react";

// A wireframe surface with a well at its centre: the enclave. A few points drift
// across the grid and sink into the well, like requests entering a sealed route.
const SPAN = 2.4; // half-width of the surface in world units
const LINES = 44;
const STEPS = 90;
const TILT = 0.62; // camera pitch in radians
const DIST = 3.3;

function height(x: number, z: number, t: number) {
  const r2 = x * x + (z - 0.15) * (z - 0.15);
  const well = -0.62 * Math.exp(-r2 / 0.22);
  const swell = 0.05 * Math.sin(2.2 * x + t * 0.35) * Math.cos(2.0 * z - t * 0.25);
  return well + swell;
}

export default function GridField() {
  const ref = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    let w = 0;
    let h = 0;
    let raf = 0;
    const cosT = Math.cos(TILT);
    const sinT = Math.sin(TILT);

    const resize = () => {
      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      w = canvas.clientWidth;
      h = canvas.clientHeight;
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    };

    const project = (x: number, y: number, z: number) => {
      // Pitch the camera down so near rows sit low on screen and the well dips away from the viewer.
      const yr = y * cosT + z * sinT;
      const zr = z * cosT - y * sinT + DIST;
      const f = Math.min(w, h * 1.6) * 0.95;
      return { sx: w / 2 + (x * f) / zr, sy: h * 0.36 - (yr * f) / zr, depth: zr };
    };

    // Drifting points: each walks a straight path toward the well and respawns at the edge.
    const seeds = Array.from({ length: 7 }, (_, i) => ({ a: (i / 7) * Math.PI * 2 + 0.4, phase: i / 7 }));

    const draw = (ms: number) => {
      const t = reduce ? 0 : ms / 1000;
      ctx.clearRect(0, 0, w, h);
      ctx.lineWidth = 1;

      const drawLine = (fixed: number, alongX: boolean) => {
        let prev: ReturnType<typeof project> | null = null;
        for (let s = 0; s <= STEPS; s++) {
          const u = -SPAN + (2 * SPAN * s) / STEPS;
          const x = alongX ? u : fixed;
          const z = alongX ? fixed : u;
          const p = project(x, height(x, z, t), z);
          if (prev) {
            const near = 1 - Math.min(1, Math.max(0, (p.depth - 1.6) / 3.2));
            const edge = 1 - Math.min(1, Math.abs(u) / SPAN) ** 3;
            ctx.strokeStyle = `rgba(236,238,240,${(0.1 + 0.5 * near) * edge})`;
            ctx.beginPath();
            ctx.moveTo(prev.sx, prev.sy);
            ctx.lineTo(p.sx, p.sy);
            ctx.stroke();
          }
          prev = p;
        }
      };

      for (let i = 0; i <= LINES; i++) {
        const v = -SPAN + (2 * SPAN * i) / LINES;
        drawLine(v, true);
        drawLine(v, false);
      }

      for (const seed of seeds) {
        const k = reduce ? seed.phase : (t * 0.06 + seed.phase) % 1;
        const r = SPAN * 0.9 * (1 - k);
        const x = Math.cos(seed.a) * r;
        const z = 0.15 + Math.sin(seed.a) * r;
        const p = project(x, height(x, z, t), z);
        const glow = ctx.createRadialGradient(p.sx, p.sy, 0, p.sx, p.sy, 10);
        glow.addColorStop(0, `rgba(201,211,255,${0.9 * (1 - k * 0.6)})`);
        glow.addColorStop(1, "rgba(201,211,255,0)");
        ctx.fillStyle = glow;
        ctx.beginPath();
        ctx.arc(p.sx, p.sy, 10, 0, Math.PI * 2);
        ctx.fill();
      }

      if (!reduce) raf = requestAnimationFrame(draw);
    };

    resize();
    raf = requestAnimationFrame(draw);
    window.addEventListener("resize", resize);
    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener("resize", resize);
    };
  }, []);

  return <canvas ref={ref} className="absolute inset-0 h-full w-full" aria-hidden="true" />;
}
