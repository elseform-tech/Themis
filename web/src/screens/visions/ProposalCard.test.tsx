// @vitest-environment jsdom
import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ChannelEntry } from "../../lib/visions";
import { ProposalCard } from "./ProposalCard";

type Proposed = Extract<ChannelEntry, { type: "proposal" }>;
type Applied = Extract<ChannelEntry, { type: "proposal_applied" }>;

const proposal: Proposed = {
  type: "proposal", vision_id: "admin", proposal_id: "p1",
  proposal: {
    summary: "A channel for shipping releases, with Rook to run it.",
    channels: [{ name: "releases", topic: "Ship releases", members: ["Rook"] }],
    visions: [{ name: "Rook", role: "Ships releases", shape: "wide", color: "rose" }],
    changes: [{ vision: "Wren", role: "Writes the notes" }, { channel: "docs", add_members: ["Rook"] }],
  },
};
const applied: Applied = {
  type: "proposal_applied", vision_id: "admin", proposal_id: "p1",
  items: [
    { kind: "vision", id: "v2", name: "Rook", action: "created" },
    { kind: "channel", id: "c2", name: "releases", action: "created" },
    { kind: "vision", id: "v1", name: "Wren", action: "changed" },
    { kind: "channel", id: "c3", name: "docs", action: "changed" },
  ],
};

describe("ProposalCard", () => {
  it("a_proposal_card_shows_what_was_created", () => {
    const open = vi.fn();
    const { rerender } = render(<ProposalCard entry={proposal} onOpenChannel={open} />);
    const card = screen.getByRole("group", { name: "Proposal" });
    expect(card).toHaveTextContent("A channel for shipping releases, with Rook to run it.");
    const items = within(card).getAllByRole("listitem").map(item => item.textContent);
    expect(items).toEqual([
      "New channel # releasesShip releases · with Rook",
      "New Vision RookShips releases",
      "WrenNew job: Writes the notes",
      "# docsAdds Rook",
    ]);
    // Nothing is set up until the person says yes.
    expect(within(card).getByRole("status")).toHaveTextContent("Waiting for your answer");
    expect(within(card).queryByRole("button")).toBeNull();

    rerender(<ProposalCard entry={proposal} applied={applied} onOpenChannel={open} />);
    const done = within(screen.getByRole("group", { name: "Proposal" })).getByRole("status");
    expect(done).toHaveTextContent("Set up: # releases, Rook, # docs, Wren");
    fireEvent.click(within(done).getByRole("button", { name: "Open # releases" }));
    expect(open).toHaveBeenCalledWith("c2");
    fireEvent.click(within(done).getByRole("button", { name: "Open # docs" }));
    expect(open).toHaveBeenLastCalledWith("c3");
  });

  it("set_up_on_its_own_lists_what_was_made_when_the_proposal_is_not_loaded", () => {
    render(<ProposalCard applied={applied} onOpenChannel={vi.fn()} />);
    const card = screen.getByRole("group", { name: "Proposal" });
    expect(within(card).getByRole("status")).toHaveTextContent("Set up: # releases, Rook, # docs, Wren");
    expect(within(card).queryByRole("list")).toBeNull();
  });
});
