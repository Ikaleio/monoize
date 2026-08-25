import type { Locale } from './i18n';

export interface HomeContent {
  tagline: string;
  description: string;
  getStarted: string;
  readDocs: string;
  features: {
    title: string;
    description: string;
  }[];
}

/**
 * Landing copy per locale. Feature order is fixed:
 * 1. protocol conversion, 2. request capture, 3. transforms, 4. reliability.
 */
export const homeContent: Record<Locale, HomeContent> = {
  en: {
    tagline: 'AI APIs look alike. Their contracts differ.',
    description:
      'Monoize is a Rust gateway for OpenAI Responses, Chat Completions, Anthropic Messages, Gemini, embeddings, and image APIs. It converts protocol semantics, routes one logical model across multiple upstream channels, and recovers from upstream failures.',
    getStarted: 'Get started',
    readDocs: 'Read the docs',
    features: [
      {
        title: 'Protocol conversion',
        description:
          'Monoize decodes each protocol into a typed canonical form and encodes it for the upstream. Text, reasoning, tool calls, images, and usage keep their roles.',
      },
      {
        title: 'Request capture',
        description:
          'Enable capture per request source. Inspect exact downstream and upstream payloads in a structured viewer.',
      },
      {
        title: 'Transforms',
        description:
          '33 built-in transforms adjust requests and responses per Provider, per API key, or globally. Model globs select where each rule applies.',
      },
      {
        title: 'Routing and reliability',
        description:
          'Monoize retries failed channels and moves forward to the next route. Fallback stops after the first response byte.',
      },
    ],
  },
  zh: {
    tagline: 'AI API 看起来相似，但协议并不相同。',
    description:
      'Monoize 是用 Rust 编写的 AI API 网关，支持 OpenAI Responses、Chat Completions、Anthropic Messages、Gemini、Embeddings 与图像 API。它转换协议语义，将一个逻辑模型路由到多个上游 Channel，并在首字节前完成故障重试与回退。',
    getStarted: '快速开始',
    readDocs: '阅读文档',
    features: [
      {
        title: '协议语义转换',
        description:
          '将输入协议解码为统一的类型化内部表示，再编码为目标上游协议。文本、推理、工具调用、图像与用量保持原有角色。',
      },
      {
        title: '请求捕获',
        description:
          '按请求来源开启捕获。在结构化查看器中检查每次尝试的下游与上游报文。',
      },
      {
        title: 'Transform 变换',
        description:
          '33 个内置 Transform 可按 Provider、API Key 或全局范围修改请求和响应。模型 glob 决定规则适用范围。',
      },
      {
        title: '路由与可靠性',
        description:
          'Monoize 重试失败的 Channel 并前进到下一条路由。发出第一个响应字节后停止回落。',
      },
    ],
  },
  'zh-TW': {
    tagline: 'AI API 看起來相似，但協議並不相同。',
    description:
      'Monoize 是一個以 Rust 撰寫的 AI API 閘道，支援 OpenAI Responses、Chat Completions、Anthropic Messages、Gemini、Embeddings 與圖像 API。它轉換協議語意，將一個邏輯模型路由到多個上游 Channel，並在首位元組前完成故障重試與備援。',
    getStarted: '快速開始',
    readDocs: '閱讀文件',
    features: [
      {
        title: '協議語意轉換',
        description:
          '將輸入協議解碼為統一的型別化內部表示，再編碼為目標上游協議。文字、推理、工具呼叫、圖像與用量保持原有角色。',
      },
      {
        title: '請求擷取',
        description:
          '依請求來源開啟擷取。在結構化檢視器中檢查每次嘗試的下游與上游封包。',
      },
      {
        title: 'Transform 變換',
        description:
          '33 個內建 Transform 可依 Provider、API 金鑰或全域範圍修改請求和回應。模型 glob 決定規則適用範圍。',
      },
      {
        title: '路由與可靠性',
        description:
          'Monoize 重試失敗的 Channel 並前進到下一條路由。送出第一個回應位元組後停止備援。',
      },
    ],
  },
  ja: {
    tagline: 'AI API は似ていても、その契約は異なります。',
    description:
      'Monoize は Rust 製の AI API ゲートウェイです。OpenAI Responses、Chat Completions、Anthropic Messages、Gemini、埋め込み、画像 API に対応します。プロトコルの意味論を変換し、1 つの論理モデルを複数の上流 Channel にルーティングし、上流障害から回復します。',
    getStarted: 'はじめる',
    readDocs: 'ドキュメントを読む',
    features: [
      {
        title: 'プロトコル変換',
        description:
          'Monoize は各プロトコルを型付きの内部表現にデコードし、上流プロトコルへエンコードします。テキスト、推論、ツール呼び出し、画像、使用量はそれぞれの役割を保持します。',
      },
      {
        title: 'リクエストキャプチャ',
        description:
          'リクエスト元ごとにキャプチャを有効化できます。構造化ビューアで各試行の下流・上流ペイロードを確認できます。',
      },
      {
        title: 'Transform 変換',
        description:
          '33 個の組み込み Transform が Provider、API キー、グローバルにリクエストとレスポンスを調整します。モデル glob で適用範囲を選択します。',
      },
      {
        title: 'ルーティングと信頼性',
        description:
          'Monoize は失敗した Channel を再試行し、次のルートへ進みます。最初のレスポンスバイト送出後はフォールバックを停止します。',
      },
    ],
  },
};

export function getHomeContent(locale: string): HomeContent {
  return homeContent[locale as Locale] ?? homeContent.en;
}
