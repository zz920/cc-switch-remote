import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ShareStatus } from "@/types/share";
import { ShareProviderRouteCard } from "@/components/share/ShareProviderRouteCard";

const mocks = vi.hoisted(() => ({
  activate: vi.fn().mockResolvedValue(undefined),
  testProvider: vi.fn().mockResolvedValue({ success: true, message: "ok" }),
  status: {
    joined: true,
    mode: "consumer",
    routePreference: "local_only",
    routeTargets: {},
    nodeName: "consumer-test",
    relayAddr: "",
    sharedProviderIds: [],
    quotaScope: "daily",
    quotaMaxTokens: 0,
    quotaPerPeer: true,
    providedTokens: 0,
    consumedTokens: 0,
    // 两个在线节点：
    // - lender-a：zhipu（有模型、有逐行用量、配了节点级配额 100000）
    // - lender-b：OpenAI Official（managed_oauth：无模型，动态模型标签）
    peers: [
      {
        peerId: "lender-a",
        name: "轻快海豚-07",
        online: true,
        direct: true,
        sharedApps: ["codex"],
        providers: [
          {
            app: "codex",
            providerId: "zhipu",
            name: "Zhipu GLM",
            models: ["glm-5.3"],
            defaultModel: "glm-5.3",
            usedTokens: 12340,
            authMode: null,
          },
        ],
        tokensUsed: 12340,
        quotaRemaining: 87660,
        isBlocked: false,
      },
      {
        peerId: "lender-b",
        name: "明亮海獭-42",
        online: true,
        direct: false,
        sharedApps: ["codex"],
        providers: [
          {
            app: "codex",
            providerId: "codex-official",
            name: "OpenAI Official",
            models: [],
            defaultModel: null,
            usedTokens: 0,
            authMode: "managed_oauth",
          },
        ],
        tokensUsed: 0,
        quotaRemaining: null,
        isBlocked: false,
      },
    ],
    incomingRequests: [],
    bridgeRunning: true,
    relayConnected: true,
    localPeerId: "consumer-peer",
  } satisfies ShareStatus,
}));

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

// t 直接插值返回 defaultValue，让数字/标签断言可见
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: Record<string, unknown>) => {
      const fallback = options?.defaultValue;
      if (typeof fallback === "string") {
        return fallback.replace(/\{\{(\w+)\}\}/g, (_m, name) =>
          String(options?.[name] ?? ""),
        );
      }
      return key;
    },
  }),
}));

vi.mock("@tanstack/react-query", () => ({
  useQueryClient: () => ({ invalidateQueries: vi.fn() }),
}));

vi.mock("@/lib/query/share", () => ({
  useShareStatus: () => ({ data: mocks.status, isLoading: false }),
  useActivateSharedProvider: () => ({
    isPending: false,
    mutateAsync: mocks.activate,
  }),
  useTestSharedProvider: () => ({
    isPending: false,
    mutateAsync: mocks.testProvider,
  }),
}));

describe("ShareProviderRouteCard", () => {
  it("suffixes entries with the lending node name to disambiguate duplicates", () => {
    render(
      <ShareProviderRouteCard
        appId="codex"
        isProxyRunning={true}
        isProxyTakeover={true}
      />,
    );

    expect(screen.getByText("@轻快海豚-07")).toBeInTheDocument();
    expect(screen.getByText("@明亮海獭-42")).toBeInTheDocument();
  });

  it("shows per-provider usage and the peer quota cap when configured", () => {
    render(
      <ShareProviderRouteCard
        appId="codex"
        isProxyRunning={true}
        isProxyTakeover={true}
      />,
    );

    const zhipuRow = screen.getByText("Zhipu GLM").closest("div.rounded-md")!;
    expect(zhipuRow.textContent).toContain("已使用: 12,340");
    expect(zhipuRow.textContent).toContain("总: 100,000");
  });

  it("hides the total segment when the lender set no quota", () => {
    render(
      <ShareProviderRouteCard
        appId="codex"
        isProxyRunning={true}
        isProxyTakeover={true}
      />,
    );

    const officialRow = screen
      .getByText("OpenAI Official")
      .closest("div.rounded-md")!;
    expect(officialRow.textContent).toContain("已使用: 0");
    expect(officialRow.textContent).not.toContain("总:");
  });

  it("labels managed oauth providers with the dynamic-model text and node-level coverage", () => {
    render(
      <ShareProviderRouteCard
        appId="codex"
        isProxyRunning={true}
        isProxyTakeover={true}
      />,
    );

    expect(screen.getAllByText("ChatGPT 账号动态模型").length).toBeGreaterThan(
      0,
    );
    // 两行都是 1/2：zhipu 的 glm-5.3 仅 lender-a 提供（按模型计 1），
    // official 按 managed_oauth 特判计 1；分母为在线节点数 2
    expect(screen.getAllByText("1/2")).toHaveLength(2);
  });
});
