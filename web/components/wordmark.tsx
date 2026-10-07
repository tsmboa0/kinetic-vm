"use client";

import { useEffect, useRef, useState, useSyncExternalStore } from "react";

import { MARK_LINES, MARK_WIDTH, VM_COLUMN } from "@/lib/glyphs";

const LETTER_MS = 18;

type WordmarkProps = {
  onDone?: () => void;
};

function subscribeReduced(onChange: () => void) {
  const media = window.matchMedia("(prefers-reduced-motion: reduce)");
  media.addEventListener("change", onChange);
  return () => media.removeEventListener("change", onChange);
}

function reducedNow() {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/** Types the terminal wordmark, one column at a time. */
export function Wordmark({ onDone }: WordmarkProps) {
  const reduce = useSyncExternalStore(subscribeReduced, reducedNow, () => false);
  const [shown, setShown] = useState(0);
  const done = useRef(onDone);

  useEffect(() => {
    done.current = onDone;
  });

  useEffect(() => {
    if (reduce) {
      done.current?.();
      return;
    }
    let column = 0;
    const timer = window.setInterval(() => {
      column += 1;
      setShown(column);
      if (column >= MARK_WIDTH) {
        window.clearInterval(timer);
        done.current?.();
      }
    }, LETTER_MS);
    return () => window.clearInterval(timer);
  }, [reduce]);

  const columns = reduce ? MARK_WIDTH : shown;
  const caret = !reduce && columns < MARK_WIDTH;

  return (
    <pre className="wordmark overflow-x-auto text-violet" aria-label="Kinetic VM">
      {MARK_LINES.map((line, row) => {
        const visible = line.slice(0, columns);
        const purple = visible.slice(0, VM_COLUMN);
        const white = visible.slice(VM_COLUMN);
        return (
          <div key={row}>
            <span>{purple}</span>
            <span className="text-foreground">{white}</span>
            {caret && row === 2 ? <span className="caret text-violet-hot">▌</span> : null}
          </div>
        );
      })}
    </pre>
  );
}
