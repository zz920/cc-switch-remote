import { useState } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import {
  NewUserGuide,
  type NewUserGuideStep,
} from "@/components/onboarding/NewUserGuide";

const steps: NewUserGuideStep[] = [
  {
    target: '[data-guide="providers"]',
    title: "Providers",
    description: "Manage your providers.",
  },
  {
    target: '[data-guide="settings"]',
    title: "Settings",
    description: "Change your preferences.",
  },
];

describe("NewUserGuide", () => {
  it("moves between steps and completes through the finish button", () => {
    const onSkip = vi.fn();
    const onFinish = vi.fn();
    render(<NewUserGuide steps={steps} onSkip={onSkip} onFinish={onFinish} />);

    expect(
      screen.getByRole("dialog", { name: "Providers" }),
    ).toHaveAccessibleDescription("Manage your providers.");
    expect(screen.getByRole("heading", { name: "Providers" })).toHaveFocus();
    fireEvent.click(screen.getByRole("button", { name: "onboarding.next" }));
    expect(screen.getByRole("heading", { name: "Settings" })).toHaveFocus();
    fireEvent.click(screen.getByRole("button", { name: "onboarding.back" }));
    expect(screen.getByRole("heading", { name: "Providers" })).toHaveFocus();
    fireEvent.click(screen.getByRole("button", { name: "onboarding.next" }));
    fireEvent.click(screen.getByRole("button", { name: "onboarding.finish" }));
    expect(onFinish).toHaveBeenCalledOnce();
    expect(onSkip).not.toHaveBeenCalled();
  });

  it("dismisses on Escape and restores focus to the opener", async () => {
    const onSkip = vi.fn();
    const onFinish = vi.fn();

    function TestApp() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button onClick={() => setOpen(true)}>Open guide</button>
          {open && (
            <NewUserGuide
              steps={steps}
              onFinish={onFinish}
              onSkip={() => {
                onSkip();
                setOpen(false);
              }}
            />
          )}
        </>
      );
    }

    render(<TestApp />);
    const opener = screen.getByRole("button", { name: "Open guide" });
    opener.focus();
    fireEvent.click(opener);
    expect(screen.getByRole("heading", { name: "Providers" })).toHaveFocus();
    fireEvent.keyDown(document, { key: "Escape" });

    expect(onSkip).toHaveBeenCalledOnce();
    expect(onFinish).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await waitFor(() => expect(opener).toHaveFocus());
  });

  it("keeps the panel in bounds after resizing and centers it if the target scrolls out", async () => {
    const onSkip = vi.fn();
    const onFinish = vi.fn();
    const highlightId = "new-user-guide-highlight";
    const target = <button data-guide="providers">Provider list</button>;
    const { rerender } = render(<div>{target}</div>);
    const targetElement = screen.getByRole("button", { name: "Provider list" });
    const rect = vi
      .spyOn(targetElement, "getBoundingClientRect")
      .mockReturnValue(new DOMRect(window.innerWidth - 27, 20, 20, 20));

    rerender(
      <div>
        {target}
        <NewUserGuide steps={steps} onSkip={onSkip} onFinish={onFinish} />
      </div>,
    );

    const panel = screen.getByRole("dialog", { name: "Providers" });
    expect(screen.getByTestId(highlightId)).toHaveStyle({ top: "14px" });
    expect(panel).toHaveStyle({ top: "58px" });
    expect(
      parseFloat(panel.style.left) + parseFloat(panel.style.width),
    ).toBeLessThan(window.innerWidth);

    vi.stubGlobal("innerWidth", 320);
    try {
      rect.mockReturnValue(new DOMRect(293, 20, 20, 20));
      fireEvent.resize(window);
      await waitFor(() => {
        expect(panel).toHaveStyle({ left: "16px", width: "288px" });
      });

      rect.mockReturnValue(new DOMRect(20, -40, 20, 20));
      fireEvent.scroll(window);
      await waitFor(() => {
        expect(screen.queryByTestId(highlightId)).not.toBeInTheDocument();
        expect(panel).toHaveStyle({ transform: "translate(-50%, -50%)" });
      });
    } finally {
      vi.unstubAllGlobals();
    }
    fireEvent.click(screen.getByRole("button", { name: "onboarding.skip" }));
    expect(onSkip).toHaveBeenCalledOnce();
  });
});
