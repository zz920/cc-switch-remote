import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ShareStatus } from "@/types/share";
import { ShareSettingsTab } from "./ShareSettingsTab";

const mocks = vi.hoisted(() => ({
  setRelayAddr: vi.fn().mockResolvedValue(undefined),
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

vi.mock("@/components/share/ShareNetworkSection", () => ({
  ApprovalCard: () => null,
  ShareNetworkSection: () => null,
}));

vi.mock("@/lib/query/queries", () => ({
  useProvidersQuery: () => ({ data: { providers: {} } }),
}));

vi.mock("@/lib/query/share", () => {
  const mutation = () => ({
    isPending: false,
    mutateAsync: vi.fn().mockResolvedValue(undefined),
  });

  return {
    useBlockPeer: mutation,
    useLeaveNetwork: mutation,
    useRegenerateKey: mutation,
    useSetNodeName: mutation,
    useSetQuota: mutation,
    useSetRelayAddr: () => ({
      isPending: false,
      mutateAsync: mocks.setRelayAddr,
    }),
    useSetRouteMode: mutation,
    useSetSharedProviders: mutation,
    useShareKeyStorage: () => ({ data: "keyring" }),
    useShareStatus: () => ({ data: mocks.status, isLoading: false }),
    useUnblockPeer: mutation,
  };
});

describe("ShareSettingsTab relay configuration", () => {
  it("allows an unjoined consumer to save a custom relay address", async () => {
    render(<ShareSettingsTab />);

    fireEvent.click(screen.getByText("Relay 与连接"));

    const input = await screen.findByPlaceholderText(
      "share.settings.relay.placeholder",
    );
    expect(input).toBeEnabled();

    const relayAddr =
      "/ip4/192.168.1.100/udp/15720/quic-v1/p2p/12D3KooWExample";
    fireEvent.change(input, { target: { value: relayAddr } });
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));

    await waitFor(() => {
      expect(mocks.setRelayAddr).toHaveBeenCalledWith(relayAddr);
    });
  });
});
