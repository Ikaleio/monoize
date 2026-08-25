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
        title: 'Near-lossless protocol conversion',
        description:
          'Monoize decodes each protocol into one typed canonical form and encodes it for the upstream. Text, reasoning, tool calls, images, and usage keep their roles.',
      },
      {
        title: 'Request capture and inspection',
        description:
          'Enable capture per request source. Inspect the exact downstream and upstream payloads of each attempt in a structured viewer.',
      },
      {
        title: 'Configurable transforms',
        description:
          '33 built-in transforms adjust requests and responses. Attach them to a Provider, an API key, or the global chain. Model globs select where each rule applies.',
      },
      {
        title: 'Retry, fallback, circuit breaker',
        description:
          'Monoize retries failed channels and falls forward to the next route. Fallback stops after the first response byte, so streams never mix two generations.',
      },
    ],
  },
  zh: {
    tagline: 'AI API 看起来相似，但协议并不相同。',
    description:
      'Monoize 是一个用 Rust 编写的 AI API 网关，支持 OpenAI Responses、Chat Completions、Anthropic Messages、Gemini、Embeddings 和图像 API。它转换协议语义，将一个逻辑模型路由到多个上游渠道，并自动处理上游故障。',
    getStarted: '快速开始',
    readDocs: '阅读文档',
    features: [
      {
        title: '近乎无损的协议转换',
        description:
          'Monoize 将每种协议解码为统一的类型化内部表示，再编码为上游协议。文本、推理、工具调用、图像和用量各自保持原有语义。',
      },
      {
        title: '请求捕获与检查',
        description:
          '按请求来源开启捕获。在结构化查看器中检查每次尝试的下游与上游原始报文。',
      },
      {
        title: '可配置的转换器',
        description:
          '33 个内置转换器可修改请求和响应。可挂载到 Provider、API 密钥或全局链上，用模型通配符选择生效范围。',
      },
      {
        title: '重试、回退与熔断',
        description:
          'Monoize 重试失败渠道并前进到下一条路由。发出第一个响应字节后停止回退，流式输出不会混入两次生成。',
      },
    ],
  },
  'zh-TW': {
    tagline: 'AI API 看起來相似，但協議並不相同。',
    description:
      'Monoize 是一個以 Rust 撰寫的 AI API 閘道，支援 OpenAI Responses、Chat Completions、Anthropic Messages、Gemini、Embeddings 與圖像 API。它轉換協議語意，將一個邏輯模型路由到多個上游渠道，並自動處理上游故障。',
    getStarted: '快速開始',
    readDocs: '閱讀文件',
    features: [
      {
        title: '近乎無損的協議轉換',
        description:
          'Monoize 將每種協議解碼為統一的型別化內部表示，再編碼為上游協議。文字、推理、工具呼叫、圖像與用量各自保持原有語意。',
      },
      {
        title: '請求擷取與檢查',
        description:
          '依請求來源開啟擷取。在結構化檢視器中檢查每次嘗試的下游與上游原始封包。',
      },
      {
        title: '可設定的轉換器',
        description:
          '33 個內建轉換器可修改請求和回應。可掛載到 Provider、API 金鑰或全域鏈上，用模型萬用字元選擇生效範圍。',
      },
      {
        title: '重試、備援與斷路器',
        description:
          'Monoize 重試失敗渠道並前進到下一條路由。送出第一個回應位元組後停止備援，串流輸出不會混入兩次生成。',
      },
    ],
  },
  ja: {
    tagline: 'AI API は似ていても、その契約は異なります。',
    description:
      'Monoize は Rust 製の AI API ゲートウェイです。OpenAI Responses、Chat Completions、Anthropic Messages、Gemini、埋め込み、画像 API に対応します。プロトコルの意味論を変換し、1 つの論理モデルを複数の上流チャネルにルーティングし、上流障害から自動的に回復します。',
    getStarted: 'はじめる',
    readDocs: 'ドキュメントを読む',
    features: [
      {
        title: 'ほぼ無損失のプロトコル変換',
        description:
          'Monoize は各プロトコルを型付きの正規表現形式にデコードし、上流プロトコルへエンコードします。テキスト、推論、ツール呼び出し、画像、使用量はそれぞれの役割を保持します。',
      },
      {
        title: 'リクエストのキャプチャと検査',
        description:
          'リクエスト元ごとにキャプチャを有効化できます。各試行の下流・上流ペイロードを構造化ビューアで確認できます。',
      },
      {
        title: '設定可能な変換ルール',
        description:
          '33 個の組み込み変換がリクエストとレスポンスを調整します。Provider、API キー、グローバルチェーンに設定し、モデルグロブで適用範囲を選択します。',
      },
      {
        title: 'リトライ・フォールバック・サーキットブレーカー',
        description:
          'Monoize は失敗したチャネルを再試行し、次のルートへ進みます。最初のレスポンスバイト送出後はフォールバックを停止し、ストリームに 2 つの生成が混ざりません。',
      },
    ],
  },
};

export function getHomeContent(locale: string): HomeContent {
  return homeContent[locale as Locale] ?? homeContent.en;
}
