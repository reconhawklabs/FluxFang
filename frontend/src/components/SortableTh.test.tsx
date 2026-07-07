import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, it, expect, vi } from "vitest";
import { SortableTh } from "./SortableTh";

describe("SortableTh", () => {
  it("shows the active direction indicator and aria-sort", () => {
    render(
      <table><thead><tr>
        <SortableTh label="Name" sortKey="name" activeKey="name" activeDir="asc" onSort={() => {}} />
      </tr></thead></table>,
    );
    const th = screen.getByRole("columnheader");
    expect(th).toHaveAttribute("aria-sort", "ascending");
    expect(screen.getByRole("button", { name: /Name/ }).textContent).toContain("↑");
  });

  it("calls onSort with its key when clicked", async () => {
    const onSort = vi.fn();
    render(
      <table><thead><tr>
        <SortableTh label="Last Seen" sortKey="last_seen" activeKey="name" activeDir="asc" onSort={onSort} />
      </tr></thead></table>,
    );
    await userEvent.click(screen.getByRole("button", { name: /Last Seen/ }));
    expect(onSort).toHaveBeenCalledWith("last_seen");
    // Inactive column: aria-sort none, no arrow.
    expect(screen.getByRole("columnheader")).toHaveAttribute("aria-sort", "none");
  });
});
