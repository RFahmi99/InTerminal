import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { CommandTape } from "./common/CommandTape";
import { Dialog } from "./common/Dialog";
import { RiskBadge } from "./common/RiskBadge";
import { ContextMenu } from "./explorer/ContextMenu";

describe("Dialog", () => {
  it("focuses inside, traps Tab, closes on Esc and restores focus", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    const { rerender } = render(
      <>
        <button>Opener</button>
      </>,
    );
    screen.getByText("Opener").focus();
    rerender(
      <>
        <button>Opener</button>
        <Dialog title="Settings" onClose={onClose} footer={<button>Done</button>}>
          <input aria-label="Name" />
        </Dialog>
      </>,
    );
    expect(screen.getByRole("dialog", { name: "Settings" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Close" })).toHaveFocus();

    await user.tab();
    expect(screen.getByLabelText("Name")).toHaveFocus();
    await user.tab();
    expect(screen.getByText("Done")).toHaveFocus();
    await user.tab();
    expect(screen.getByRole("button", { name: "Close" })).toHaveFocus();

    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledOnce();

    rerender(
      <>
        <button>Opener</button>
      </>,
    );
    expect(screen.getByText("Opener")).toHaveFocus();
  });
});

describe("ContextMenu", () => {
  it("moves with arrow keys and chooses with Enter", async () => {
    const user = userEvent.setup();
    const rename = vi.fn();
    const onClose = vi.fn();
    render(
      <ContextMenu
        label="Actions for notes.txt"
        x={10}
        y={10}
        onClose={onClose}
        items={[
          { label: "Open", onSelect: vi.fn() },
          { label: "Rename", shortcut: "F2", onSelect: rename },
        ]}
      />,
    );
    expect(screen.getByRole("menuitem", { name: /Open/ })).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("menuitem", { name: /Rename/ })).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("menuitem", { name: /Open/ })).toHaveFocus();
    await user.keyboard("{ArrowUp}{Enter}");
    expect(rename).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalled();
  });
});

describe("CommandTape and RiskBadge", () => {
  it("labels commands for screen readers and marks placeholders", () => {
    render(<CommandTape command="Expand-Archive -Path <archive>" />);
    const code = screen.getByText(/Expand-Archive/).closest("code")!;
    expect(code).toHaveTextContent("Command: Expand-Archive -Path <archive>");
    expect(screen.getByText("<archive>")).toHaveClass("placeholder");
  });

  it("states risk in words, not colour alone, with reasons", () => {
    render(<RiskBadge risk={{ level: "danger", reasons: ["Deletes files permanently."] }} showReasons />);
    expect(screen.getByText("Danger")).toBeInTheDocument();
    expect(screen.getByRole("listitem")).toHaveTextContent("Deletes files permanently.");
  });
});
