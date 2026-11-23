import { useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { DialogDescription, DialogTitle } from "@/components/ui/dialog";

export interface NewUserGuideStep {
  target?: string;
  title: string;
  description: string;
}

interface NewUserGuideProps {
  steps: readonly NewUserGuideStep[];
  onSkip: () => void;
  onFinish: () => void;
}

interface Bounds {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

const MARGIN = 16;
const HIGHLIGHT_PADDING = 6;
const GAP = 12;
const MAX_PANEL_WIDTH = 368;

function findTarget(selector?: string) {
  try {
    return selector ? document.querySelector(selector) : null;
  } catch {
    return null;
  }
}

function getViewport(): Bounds {
  const viewport = window.visualViewport;
  const left = viewport?.offsetLeft ?? 0;
  const top = viewport?.offsetTop ?? 0;
  return {
    left,
    top,
    right: left + (viewport?.width ?? window.innerWidth),
    bottom: top + (viewport?.height ?? window.innerHeight),
  };
}

function getHighlight(target: Element | null, viewport: Bounds): Bounds | null {
  if (!target) return null;

  const { visibility } = window.getComputedStyle(target);
  if (visibility === "hidden" || visibility === "collapse") return null;

  const rect = target.getBoundingClientRect();
  const visible = {
    top: Math.max(rect.top, viewport.top),
    right: Math.min(rect.right, viewport.right),
    bottom: Math.min(rect.bottom, viewport.bottom),
    left: Math.max(rect.left, viewport.left),
  };

  // The provider list and settings can scroll inside the main window.
  for (
    let parent = target.parentElement;
    parent && parent !== document.body;
    parent = parent.parentElement
  ) {
    const { overflow, overflowX, overflowY } = window.getComputedStyle(parent);
    const clipsX = /^(auto|scroll|hidden|clip)$/.test(overflowX || overflow);
    const clipsY = /^(auto|scroll|hidden|clip)$/.test(overflowY || overflow);
    if (!clipsX && !clipsY) continue;

    const clipping = parent.getBoundingClientRect();
    if (clipsX) {
      visible.left = Math.max(visible.left, clipping.left + parent.clientLeft);
      visible.right = Math.min(
        visible.right,
        clipping.left + parent.clientLeft + parent.clientWidth,
      );
    }
    if (clipsY) {
      visible.top = Math.max(visible.top, clipping.top + parent.clientTop);
      visible.bottom = Math.min(
        visible.bottom,
        clipping.top + parent.clientTop + parent.clientHeight,
      );
    }
  }

  if (visible.right <= visible.left || visible.bottom <= visible.top)
    return null;

  return {
    top: Math.max(viewport.top, visible.top - HIGHLIGHT_PADDING),
    right: Math.min(viewport.right, visible.right + HIGHLIGHT_PADDING),
    bottom: Math.min(viewport.bottom, visible.bottom + HIGHLIGHT_PADDING),
    left: Math.max(viewport.left, visible.left - HIGHLIGHT_PADDING),
  };
}

function getPanelStyle(
  viewport: Bounds,
  highlight: Bounds | null,
  panelHeight: number,
): CSSProperties {
  const viewportWidth = viewport.right - viewport.left;
  const viewportHeight = viewport.bottom - viewport.top;
  const width = Math.min(
    MAX_PANEL_WIDTH,
    Math.max(0, viewportWidth - MARGIN * 2),
  );
  const maxHeight = Math.max(0, viewportHeight - MARGIN * 2);
  const centered: CSSProperties = {
    width,
    maxHeight,
    left: viewport.left + viewportWidth / 2,
    top: viewport.top + viewportHeight / 2,
    transform: "translate(-50%, -50%)",
  };
  if (!highlight) return centered;

  const above = highlight.top - viewport.top - MARGIN - GAP;
  const below = viewport.bottom - highlight.bottom - MARGIN - GAP;
  const placeBelow = below >= panelHeight || below >= above;
  const available = placeBelow ? below : above;
  if (available < Math.min(panelHeight, 160) || available <= 0) return centered;

  const leftEdge = viewport.left + MARGIN;
  const rightEdge = viewport.right - MARGIN - width;
  const centeredOnTarget = (highlight.left + highlight.right - width) / 2;
  return {
    width,
    maxHeight: available,
    left: Math.min(Math.max(centeredOnTarget, leftEdge), rightEdge),
    top: placeBelow ? highlight.bottom + GAP : highlight.top - GAP,
    transform: placeBelow ? undefined : "translateY(-100%)",
  };
}

export function NewUserGuide({ steps, onSkip, onFinish }: NewUserGuideProps) {
  const { t } = useTranslation();
  const [index, setIndex] = useState(0);
  const currentIndex = Math.min(index, steps.length - 1);
  const step = steps[currentIndex];
  const selector = step?.target;
  const panelRef = useRef<HTMLDivElement>(null);
  const titleRef = useRef<HTMLHeadingElement>(null);
  const previousFocusRef = useRef<HTMLElement | null>(null);
  const [layout, setLayout] = useState(() => ({
    viewport: getViewport(),
    highlight: null as Bounds | null,
    panelHeight: 0,
  }));

  useLayoutEffect(() => {
    let frame = 0;
    let target: Element | null = null;
    let panel: HTMLDivElement | null = null;
    const resizeObserver = new ResizeObserver(schedule);
    const mutationObserver = new MutationObserver(schedule);
    const visualViewport = window.visualViewport;

    function measure() {
      const nextTarget = findTarget(selector);
      if (nextTarget !== target) {
        if (target) resizeObserver.unobserve(target);
        if (nextTarget) resizeObserver.observe(nextTarget);
        target = nextTarget;
      }

      if (panelRef.current !== panel) {
        if (panel) resizeObserver.unobserve(panel);
        panel = panelRef.current;
        if (panel) resizeObserver.observe(panel);
      }

      const viewport = getViewport();
      const highlight = getHighlight(target, viewport);
      const panelHeight = panel
        ? panel.scrollHeight + panel.offsetHeight - panel.clientHeight
        : 0;
      setLayout({ viewport, highlight, panelHeight });
    }

    function schedule() {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(measure);
    }

    mutationObserver.observe(document.body, { childList: true, subtree: true });
    window.addEventListener("resize", schedule);
    window.addEventListener("scroll", schedule, true);
    visualViewport?.addEventListener("resize", schedule);
    visualViewport?.addEventListener("scroll", schedule);
    measure();
    schedule();

    return () => {
      cancelAnimationFrame(frame);
      resizeObserver.disconnect();
      mutationObserver.disconnect();
      window.removeEventListener("resize", schedule);
      window.removeEventListener("scroll", schedule, true);
      visualViewport?.removeEventListener("resize", schedule);
      visualViewport?.removeEventListener("scroll", schedule);
    };
  }, [selector]);

  useLayoutEffect(() => {
    if (!previousFocusRef.current) return;
    if (panelRef.current) panelRef.current.scrollTop = 0;
    titleRef.current?.focus({ preventScroll: true });
  }, [currentIndex]);

  if (!step) return null;

  const { viewport, highlight, panelHeight } = layout;
  const last = currentIndex === steps.length - 1;
  const panelStyle = getPanelStyle(viewport, highlight, panelHeight);

  return (
    <DialogPrimitive.Root
      open
      onOpenChange={(open) => {
        if (!open) onSkip();
      }}
    >
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="fixed inset-0 z-[120] overflow-hidden">
          {highlight ? (
            <div
              aria-hidden="true"
              data-testid="new-user-guide-highlight"
              className="pointer-events-none fixed rounded-lg border-2 border-primary shadow-[0_0_0_100vmax_rgba(0,0,0,0.58)]"
              style={{
                top: highlight.top,
                left: highlight.left,
                width: highlight.right - highlight.left,
                height: highlight.bottom - highlight.top,
              }}
            />
          ) : (
            <div className="absolute inset-0 bg-black/60" />
          )}
        </DialogPrimitive.Overlay>
        <DialogPrimitive.Content
          ref={panelRef}
          className="fixed z-[121] overflow-auto rounded-xl border border-border-default bg-background p-4 text-foreground shadow-xl sm:p-5"
          style={panelStyle}
          onInteractOutside={(event) => event.preventDefault()}
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            const previouslyFocused = document.activeElement;
            if (previouslyFocused instanceof HTMLElement) {
              previousFocusRef.current = previouslyFocused;
            }
            titleRef.current?.focus({ preventScroll: true });
          }}
          onCloseAutoFocus={(event) => {
            event.preventDefault();
            previousFocusRef.current?.focus({ preventScroll: true });
          }}
        >
          <p
            className="mb-2 text-xs font-medium tabular-nums text-muted-foreground"
            aria-live="polite"
          >
            {t("onboarding.progress", {
              current: currentIndex + 1,
              total: steps.length,
            })}
          </p>
          <DialogTitle
            ref={titleRef}
            tabIndex={-1}
            className="focus:outline-none"
          >
            {step.title}
          </DialogTitle>
          <DialogDescription className="mt-2 leading-relaxed">
            {step.description}
          </DialogDescription>
          <div className="mt-4 flex flex-wrap items-center justify-between gap-2">
            <Button type="button" variant="ghost" size="sm" onClick={onSkip}>
              {t("onboarding.skip")}
            </Button>
            <div className="flex flex-wrap items-center gap-2">
              {currentIndex > 0 && (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => setIndex(currentIndex - 1)}
                >
                  {t("onboarding.back")}
                </Button>
              )}
              <Button
                type="button"
                size="sm"
                onClick={last ? onFinish : () => setIndex(currentIndex + 1)}
              >
                {t(last ? "onboarding.finish" : "onboarding.next")}
              </Button>
            </div>
          </div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
