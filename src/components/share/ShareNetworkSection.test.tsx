import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ShareStatus } from "@/types/share";
import { ShareNetworkSection } from "./ShareNetworkSection";

const mocks = vi.hoisted(() => ({
  requestJoin: vi.fn().mockResolvedValue({
    shortCode: "123456",
    expiresAt: 1_800_000_000,
  }),
  status: {
    joined: false,
    routePreference: "local_only",
    mode: "consumer",
    routeTargets: {},
    nodeName: "consumer-test",
    relayAddr: "",
    sharedProviderIds: [],
    quotaScope: "daily",
    quotaMaxTokens: 0,
    quotaPerPeer: true,
    providedTokens: 0,
    consumedTokens: 0,
    peers: [],
    incomingRequests: [],
    bridgeRunning: false,
    relayConnected: false,
    localPeerId: "consumer-peer",
  } satisfies ShareStatus,
}));

vi.mock("sonner", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}));

vi.mock("@/lib/query/share", () => {
  const mutation = () => ({
    isPending: false,
    mutateAsync: vi.fn().mockResolvedValue(undefined),
  });

  return {
    useApproveJoin: mutation,
    useCancelJoin: mutation,
    useCreateNetwork: mutation,
    useLeaveNetwork: mutation,
    useRejectJoin: mutation,
    useRequestJoin: () => ({
      isPending: false,
      mutateAsync: mocks.requestJoin,
    }),
    useShareStatus: () => ({ data: mocks.status, isLoading: false }),
  };
});

describe("ShareNetworkSection join flow", () => {
  it("submits and persists a relay address with the join request", async () => {
    render(<ShareNetworkSection />);

    fireEvent.click(screen.getByRole("button", { name: "share.join.button" }));

    fireEvent.change(await screen.findByLabelText("share.join.shareIdLabel"), {
      target: { value: "PD3W-RES7" },
    });
    const relayAddr = "/ip4/192.168.1.100/tcp/15720/p2p/12D3KooWExample";
    fireEvent.change(screen.getByLabelText("share.join.relayAddrLabel"), {
      target: { value: relayAddr },
    });
    fireEvent.click(screen.getByRole("button", { name: "share.join.submit" }));

    await waitFor(() => {
      expect(mocks.requestJoin).toHaveBeenCalledWith({
        shareId: "PD3W-RES7",
        relayAddr,
      });
    });
  });
});
