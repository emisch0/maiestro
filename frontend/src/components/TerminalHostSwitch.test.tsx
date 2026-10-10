import { describe, it, expect, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { TerminalHostSwitch, TerminalHostSwitchProps } from "./TerminalHostSwitch";

function props(over: Partial<TerminalHostSwitchProps> = {}): TerminalHostSwitchProps {
  return {
    value: null,
    options: ["vscode", "terminal_app"],
    available: ["vscode", "terminal_app"],
    fallback: "vscode",
    onSwitch: vi.fn().mockResolvedValue({ status: "done", moved: 0 }),
    onSaved: vi.fn(),
    onOpenPermissionSettings: vi.fn(),
    ...over,
  };
}

const choose = (value: string) => fireEvent.change(screen.getByRole("combobox", { name: "Terminal host" }), { target: { value } });

describe("TerminalHostSwitch", () => {
  it("saves straight away when no window is open", async () => {
    const p = props();
    render(<TerminalHostSwitch {...p} />);
    choose("terminal_app");
    await waitFor(() => expect(p.onSaved).toHaveBeenCalledWith("terminal_app"));
    expect(p.onSwitch).toHaveBeenCalledWith("terminal_app", false);
  });

  it("warns about open windows and switches only once confirmed", async () => {
    const onSwitch = vi
      .fn()
      .mockResolvedValueOnce({ status: "needs_confirmation", open: ["🍋 #243 — Terminal host"] })
      .mockResolvedValueOnce({ status: "done", moved: 1 });
    const p = props({ onSwitch });
    render(<TerminalHostSwitch {...p} />);
    choose("terminal_app");
    expect(await screen.findByText("🍋 #243 — Terminal host")).toBeInTheDocument();
    expect(screen.getByText(/moves this repo's existing sessions too/)).toBeInTheDocument();
    expect(p.onSaved).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Close windows and switch" }));
    await waitFor(() => expect(p.onSaved).toHaveBeenCalledWith("terminal_app"));
    expect(onSwitch).toHaveBeenLastCalledWith("terminal_app", true);
  });

  it("changes nothing when the warning is cancelled", async () => {
    const p = props({ onSwitch: vi.fn().mockResolvedValue({ status: "needs_confirmation", open: ["t"] }) });
    render(<TerminalHostSwitch {...p} />);
    choose("terminal_app");
    fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
    expect(screen.queryByText(/moves this repo's existing sessions/)).toBeNull();
    expect(p.onSaved).not.toHaveBeenCalled();
  });

  it("offers the missing grant when a window won't close", async () => {
    const p = props({
      onSwitch: vi.fn().mockResolvedValue({
        status: "blocked_by_editor",
        message: "The Terminal window is still open.",
        permission: "automation",
      }),
    });
    render(<TerminalHostSwitch {...p} />);
    choose("vscode");
    fireEvent.click(await screen.findByRole("button", { name: "Open Automation Options" }));
    expect(p.onOpenPermissionSettings).toHaveBeenCalledWith("automation");
    expect(p.onSaved).not.toHaveBeenCalled();
  });

  it("as the global default, shows the resolved host and names who moves", async () => {
    const p = props({
      value: null,
      fallback: "terminal_app",
      defaultOption: false,
      scope: "the existing sessions of every repo that uses the default",
      onSwitch: vi.fn().mockResolvedValue({ status: "needs_confirmation", open: ["t"] }),
    });
    render(<TerminalHostSwitch {...p} />);
    const select = screen.getByRole("combobox", { name: "Terminal host" }) as HTMLSelectElement;
    expect(select.value).toBe("terminal_app");
    expect(screen.queryByRole("option", { name: /Default/ })).toBeNull();
    choose("vscode");
    expect(await screen.findByText(/moves the existing sessions of every repo that uses the default/)).toBeInTheDocument();
  });
});
