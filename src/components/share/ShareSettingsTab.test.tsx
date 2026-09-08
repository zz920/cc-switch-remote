import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ShareStatus } from "@/types/share";
import { ShareSettingsTab } from "./ShareSettingsTab";

const mocks = vi.hoisted(() => ({
  setRelayAddr: vi.fn().mockResolvedValue(undefined),
  providersByApp: {} as Record<string, Record<string, unknown>>,
  status: {
    joined: false as boolean,
    routePreference: "local_only",
    mode: "consumer" as ShareStatus["mode"],
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
  useProvidersQuery: (appId: string) => ({
    data: { providers: mocks.providersByApp[appId] ?? {} },
  }),
}));

vi.mock("@/components/providers/forms/hooks/useCodexOauth", () => ({
  useCodexOauth: () => ({
    accounts: [],
    isStatusSuccess: true,
    isAuthenticated: false,
    defaultAccountId: null,
  }),
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

describe("ShareSettingsTab shared providers official gating", () => {
  beforeEach(() => {
    mocks.providersByApp = {
      codex: {
        "codex-official": {
          id: "codex-official",
          name: "OpenAI Official",
          category: "official",
          meta: {},
        },
      },
      claude: {
        "claude-official": {
          id: "claude-official",
          name: "Claude Official",
          category: "official",
          meta: {},
        },
      },
    };
    mocks.status.joined = true;
    mocks.status.mode = "provider";
    mocks.status.peers = [];
  });

  afterEach(() => {
    mocks.status.joined = false;
    mocks.status.mode = "consumer";
    mocks.providersByApp = {};
  });

  it("offers bind/login CTA only on codex official, plain badge elsewhere", async () => {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    render(
      <QueryClientProvider client={client}>
        <ShareSettingsTab />
      </QueryClientProvider>,
    );

    fireEvent.click(screen.getByText("共享与配额"));

    const codexRow = (await screen.findByText("OpenAI Official")).closest(
      "label",
    )!;
    // 未绑定的 OpenAI Official：行动按钮（无可用账号 → 去登录文案键）
    expect(codexRow.querySelector("button")?.textContent).toBe(
      "share.settings.sharedProviders.goLogin",
    );

    // 其他应用的 Official：只有"不可共享"徽标，没有任何按钮
    const claudeRow = screen.getByText("Claude Official").closest("label")!;
    expect(claudeRow.textContent).toContain(
      "share.settings.sharedProviders.officialBadge",
    );
    expect(claudeRow.querySelector("button")).toBeNull();
  });
});
