"use client";

import { motion, useReducedMotion } from "framer-motion";
import Link from "next/link";
import { useEffect, useState } from "react";

import { Atmosphere } from "@/components/storm";
import { Wordmark } from "@/components/wordmark";
import { claimHref, links } from "@/lib/links";

/* ─────────────────────────────────────────────────────────
 * LANDING STORYBOARD
 *
 * Nav, including Claim, is usable immediately.
 *
 *    0ms   wordmark types in the center of the viewport
 *  type   tagline rises under the mark
 * +180ms  the two actions spring in
 * +360ms  the quiet line under the actions
 * ───────────────────────────────────────────────────────── */

const TIMING = {
  actions: 180,
  line: 360,
} as const;

const RISE = { type: "spring" as const, stiffness: 320, damping: 28 };

export function Landing({ host }: { host: string }) {
  const reduce = useReducedMotion();
  const [stage, setStage] = useState(0);
  const claim = claimHref(host);

  useEffect(() => {
    if (stage < 1 || reduce) return;
    const actions = window.setTimeout(() => setStage((current) => Math.max(current, 2)), TIMING.actions);
    const line = window.setTimeout(() => setStage((current) => Math.max(current, 3)), TIMING.line);
    return () => {
      window.clearTimeout(actions);
      window.clearTimeout(line);
    };
  }, [stage, reduce]);

  return (
    <div className="relative min-h-dvh">
      <Atmosphere />
      <header className="absolute inset-x-0 top-0 z-10 flex items-center justify-between gap-4 px-5 py-4 sm:px-10">
        <Link href="/" className="brand">
          KINETIC
        </Link>
        <nav className="flex items-center">
          <a className="nav-link" href={links.docs}>
            Docs
          </a>
          <a className="nav-link hidden sm:inline-flex" href={links.x}>
            X
          </a>
          <a className="nav-link hidden sm:inline-flex" href={links.github}>
            GitHub
          </a>
          <ClaimLink href={claim} className="btn-white ml-2">
            Claim
          </ClaimLink>
        </nav>
      </header>

      <main className="flex min-h-dvh flex-col items-center justify-center px-6 pb-16 pt-24 text-center">
        <Wordmark onDone={() => setStage(reduce ? 3 : 1)} />
        <motion.p
          className="mt-10 max-w-xl text-lg leading-8 text-dim sm:text-xl"
          initial={reduce ? false : { opacity: 0, y: 14 }}
          animate={{ opacity: stage >= 1 ? 1 : 0, y: stage >= 1 ? 0 : 14 }}
          transition={RISE}
        >
          Infrastructure that connects every physical AI system to the Monad blockchain.
        </motion.p>
        <motion.div
          className="mt-8 flex flex-wrap items-center justify-center gap-3"
          initial={reduce ? false : { opacity: 0, y: 16 }}
          animate={{ opacity: stage >= 2 ? 1 : 0, y: stage >= 2 ? 0 : 16 }}
          transition={RISE}
        >
          <ClaimLink href={claim} className="btn-white">
            Claim a device
          </ClaimLink>
          <a href={links.docs} className="btn-ghost">
            Read the docs
          </a>
        </motion.div>
        <motion.p
          className="mt-8 text-sm text-dim"
          initial={reduce ? false : { opacity: 0, y: 10 }}
          animate={{ opacity: stage >= 3 ? 1 : 0, y: stage >= 3 ? 0 : 10 }}
          transition={RISE}
        >
          The device signs. The person holds the identity.
        </motion.p>
      </main>
    </div>
  );
}

function ClaimLink({ href, className, children }: { href: string; className: string; children: string }) {
  if (href.startsWith("/")) {
    return (
      <Link href={href} className={className}>
        {children}
      </Link>
    );
  }
  return (
    <a href={href} className={className}>
      {children}
    </a>
  );
}
