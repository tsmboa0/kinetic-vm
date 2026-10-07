"use client";

import { useEffect, useRef } from "react";

type Point = { x: number; y: number };
type Bolt = { points: Point[]; life: number; thick: boolean };
type Drop = { x: number; y: number; speed: number; length: number; alpha: number };

const STRIKE = "kinetic-strike";

export function strike() {
  window.dispatchEvent(new Event(STRIKE));
}

/** Night sky: drifting fog, rain, and lightning that actually lights the page. */
export function Atmosphere() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const flashRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const context = canvas.getContext("2d");
    if (!context) return;
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

    let frame = 0;
    let width = 0;
    let height = 0;
    const bolts: Bolt[] = [];
    const drops: Drop[] = [];
    let nextBolt = 12;

    const resize = () => {
      const ratio = Math.min(window.devicePixelRatio || 1, 2);
      width = window.innerWidth;
      height = window.innerHeight;
      canvas.width = Math.floor(width * ratio);
      canvas.height = Math.floor(height * ratio);
      canvas.style.width = `${width}px`;
      canvas.style.height = `${height}px`;
      context.setTransform(ratio, 0, 0, ratio, 0, 0);
      if (drops.length === 0) seedRain();
    };

    const seedRain = () => {
      drops.length = 0;
      const count = Math.round((width * height) / 14000);
      for (let index = 0; index < count; index += 1) {
        drops.push({
          x: Math.random() * width,
          y: Math.random() * height,
          speed: 9 + Math.random() * 16,
          length: 12 + Math.random() * 22,
          alpha: 0.12 + Math.random() * 0.28,
        });
      }
    };

    const flash = (hard: boolean) => {
      const veil = flashRef.current;
      if (!veil) return;
      veil.style.opacity = hard ? "0.85" : "0.45";
      window.setTimeout(() => {
        veil.style.opacity = "0";
      }, hard ? 70 : 50);
    };

    const spawn = (hard = false) => {
      const startX = width * (0.12 + Math.random() * 0.76);
      const points: Point[] = [{ x: startX, y: -20 }];
      let x = startX;
      let y = -20;
      const end = height * (0.62 + Math.random() * 0.28);
      while (y < end) {
        y += 18 + Math.random() * 36;
        x += (Math.random() - 0.5) * 70;
        points.push({ x, y });
        if (points.length > 4 && Math.random() < 0.28) {
          const fork = points[points.length - 1];
          const branch: Point[] = [{ x: fork.x, y: fork.y }];
          let bx = fork.x;
          let by = fork.y;
          const steps = 3 + Math.floor(Math.random() * 4);
          for (let step = 0; step < steps; step += 1) {
            by += 16 + Math.random() * 22;
            bx += 10 + Math.random() * 36;
            branch.push({ x: bx, y: by });
          }
          bolts.push({ points: branch, life: 1, thick: false });
        }
      }
      bolts.push({ points, life: 1, thick: true });
      flash(hard || Math.random() < 0.4);
    };

    const drawBolt = (points: Point[], life: number, thick: boolean) => {
      const stroke = (line: number, color: string) => {
        context.beginPath();
        context.lineWidth = line;
        context.strokeStyle = color;
        context.globalAlpha = life;
        context.lineJoin = "round";
        context.lineCap = "round";
        points.forEach((point, index) => {
          if (index === 0) context.moveTo(point.x, point.y);
          else context.lineTo(point.x, point.y);
        });
        context.stroke();
      };
      if (thick) stroke(22, "rgba(168, 85, 247, 0.35)");
      stroke(thick ? 8 : 3, "rgba(196, 140, 255, 0.7)");
      stroke(thick ? 2.4 : 1.2, "rgba(255, 250, 255, 0.95)");
      context.globalAlpha = 1;
    };

    const draw = () => {
      context.clearRect(0, 0, width, height);

      const horizon = context.createLinearGradient(0, height * 0.55, 0, height);
      horizon.addColorStop(0, "rgba(109, 40, 217, 0)");
      horizon.addColorStop(0.7, "rgba(109, 40, 217, 0.08)");
      horizon.addColorStop(1, "rgba(168, 85, 247, 0.22)");
      context.fillStyle = horizon;
      context.fillRect(0, height * 0.55, width, height * 0.45);

      context.lineWidth = 1;
      for (const drop of drops) {
        drop.y += drop.speed;
        drop.x -= drop.speed * 0.08;
        if (drop.y > height + 20) {
          drop.y = -30;
          drop.x = Math.random() * width;
        }
        context.strokeStyle = `rgba(233, 213, 255, ${drop.alpha})`;
        context.beginPath();
        context.moveTo(drop.x, drop.y);
        context.lineTo(drop.x - 3, drop.y + drop.length);
        context.stroke();
      }

      const glow = 0.25 + Math.sin(frame / 18) * 0.12;
      context.strokeStyle = `rgba(233, 213, 255, ${glow})`;
      context.lineWidth = 1.5;
      context.beginPath();
      const base = height * 0.9;
      context.moveTo(0, base);
      for (let x = 0; x <= width; x += 28) {
        const y = base + Math.sin(x * 0.02 + frame * 0.08) * 6 + (Math.random() - 0.5) * 3;
        context.lineTo(x, y);
      }
      context.stroke();

      for (let index = bolts.length - 1; index >= 0; index -= 1) {
        const bolt = bolts[index];
        bolt.life -= 0.018;
        if (bolt.life <= 0) {
          bolts.splice(index, 1);
          continue;
        }
        drawBolt(bolt.points, bolt.life, bolt.thick);
      }
    };

    resize();
    window.addEventListener("resize", resize);
    if (reduce) {
      return () => window.removeEventListener("resize", resize);
    }

    const onStrike = () => spawn(true);
    window.addEventListener(STRIKE, onStrike);
    const onPointer = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Element)) return;
      if (target.closest("a, button, input, textarea")) return;
      spawn(false);
    };
    window.addEventListener("pointerdown", onPointer);

    const loop = () => {
      frame += 1;
      if (frame > nextBolt) {
        spawn(false);
        nextBolt = frame + 70 + Math.floor(Math.random() * 110);
      }
      draw();
      raf = window.requestAnimationFrame(loop);
    };
    let raf = window.requestAnimationFrame(loop);
    return () => {
      window.cancelAnimationFrame(raf);
      window.removeEventListener("resize", resize);
      window.removeEventListener(STRIKE, onStrike);
      window.removeEventListener("pointerdown", onPointer);
    };
  }, []);

  return (
    <div className="atmosphere" aria-hidden>
      <div className="sky" />
      <div className="fog fog-a" />
      <div className="fog fog-b" />
      <div className="fog fog-c" />
      <canvas ref={canvasRef} className="storm" />
      <div ref={flashRef} className="flash" />
      <div className="vignette" />
    </div>
  );
}
