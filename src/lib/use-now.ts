"use client";
import { useSyncExternalStore } from "react";

// A one-second clock for countdowns and ages. It reads 0 during prerendering and hydration, so
// time-dependent text only appears once the page is running in the browser.
let clock = 0;
function subscribeClock(onTick: () => void) {
  clock = Date.now();
  const timer = setInterval(() => { clock = Date.now(); onTick(); }, 1000);
  return () => clearInterval(timer);
}
export const useNow = () => useSyncExternalStore(subscribeClock, () => clock, () => 0);
