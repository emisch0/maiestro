import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { DismissibleError } from "./DismissibleError";

describe("DismissibleError", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("copies the lead and the full message", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    const onDismiss = vi.fn();

    render(<DismissibleError lead="Couldn't open" message={"code: not found\nsecond line"} onDismiss={onDismiss} />);
    screen.getByRole("button", { name: "Copy" }).click();

    await waitFor(() => expect(writeText).toHaveBeenCalledWith("Couldn't open\ncode: not found\nsecond line"));
    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
    expect(onDismiss).not.toHaveBeenCalled();

    screen.getByRole("button", { name: "Dismiss" }).click();
    expect(onDismiss).toHaveBeenCalledOnce();
  });
});
