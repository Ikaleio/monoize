import Image from 'next/image';
import type { BaseLayoutProps } from 'fumadocs-ui/layouts/shared';
import { uiTranslations } from 'fumadocs-ui/i18n';
import { i18n, type Locale } from './i18n';
import { appName, gitConfig } from './shared';

export const translations = i18n
  .translations()
  .extend(uiTranslations())
  .add({
    en: {
      displayName: 'English',
    },
    zh: {
      displayName: '简体中文',
      'Search(search dialog)': '搜索',
      'Search(search trigger)': '搜索文档',
      'No results found(search dialog)': '未找到结果',
      'Open Search(search trigger)(aria-label)': '打开搜索',
      'Close Search(search dialog)(aria-label)': '关闭搜索',
      'On this page(table of contents)': '本页目录',
      'No Headings(table of contents)': '无标题',
      'Table of Contents(inline table of contents)': '目录',
      'Next Page(pagination)': '下一页',
      'Previous Page(pagination)': '上一页',
      'Last updated on(page footer)': '最后更新于',
      'Edit on GitHub(edit page)': '在 GitHub 上编辑',
      'Choose a language(language switcher)': '选择语言',
      'Choose a language(language switcher)(aria-label)': '选择语言',
      'Toggle Theme(theme switcher)(aria-label)': '切换主题',
      'Light(theme switcher)(aria-label)': '浅色',
      'Dark(theme switcher)(aria-label)': '深色',
      'System(theme switcher)(aria-label)': '跟随系统',
      'Page Not Found(404 not found page)': '页面不存在',
      'Back to Home(404 not found page)': '返回首页',
      'The page you are looking for might have been removed, had its name changed, or is temporarily unavailable.(404 not found page)':
        '你访问的页面可能已被移除、重命名或暂时不可用。',
      'Copy Markdown(page actions)': '复制 Markdown',
      'View as Markdown(page actions)': '以 Markdown 查看',
      'Open in GitHub(page actions)': '在 GitHub 中打开',
      'Open(page actions)': '打开',
      'Toggle Menu(home layout header)(aria-label)': '切换菜单',
      'Open Sidebar(aria-label)': '打开侧边栏',
      'Open Sidebar(sidebar)(aria-label)': '打开侧边栏',
      'Close Sidebar(aria-label)': '关闭侧边栏',
      'Close Sidebar(sidebar)(aria-label)': '关闭侧边栏',
      'Collapse Sidebar(sidebar)(aria-label)': '收起侧边栏',
      'Hide Sidebar(sidebar)': '隐藏侧边栏',
      'Show Sidebar(sidebar)': '显示侧边栏',
      'Copy Text(code block)(aria-label)': '复制代码',
      'Copied Text(code block)(aria-label)': '已复制',
    },
    'zh-TW': {
      displayName: '繁體中文',
      'Search(search dialog)': '搜尋',
      'Search(search trigger)': '搜尋文件',
      'No results found(search dialog)': '未找到結果',
      'Open Search(search trigger)(aria-label)': '開啟搜尋',
      'Close Search(search dialog)(aria-label)': '關閉搜尋',
      'On this page(table of contents)': '本頁目錄',
      'No Headings(table of contents)': '無標題',
      'Table of Contents(inline table of contents)': '目錄',
      'Next Page(pagination)': '下一頁',
      'Previous Page(pagination)': '上一頁',
      'Last updated on(page footer)': '最後更新於',
      'Edit on GitHub(edit page)': '在 GitHub 上編輯',
      'Choose a language(language switcher)': '選擇語言',
      'Choose a language(language switcher)(aria-label)': '選擇語言',
      'Toggle Theme(theme switcher)(aria-label)': '切換主題',
      'Light(theme switcher)(aria-label)': '淺色',
      'Dark(theme switcher)(aria-label)': '深色',
      'System(theme switcher)(aria-label)': '跟隨系統',
      'Page Not Found(404 not found page)': '頁面不存在',
      'Back to Home(404 not found page)': '返回首頁',
      'The page you are looking for might have been removed, had its name changed, or is temporarily unavailable.(404 not found page)':
        '你造訪的頁面可能已被移除、重新命名或暫時無法使用。',
      'Copy Markdown(page actions)': '複製 Markdown',
      'View as Markdown(page actions)': '以 Markdown 檢視',
      'Open in GitHub(page actions)': '在 GitHub 中開啟',
      'Open(page actions)': '開啟',
      'Toggle Menu(home layout header)(aria-label)': '切換選單',
      'Open Sidebar(aria-label)': '開啟側邊欄',
      'Open Sidebar(sidebar)(aria-label)': '開啟側邊欄',
      'Close Sidebar(aria-label)': '關閉側邊欄',
      'Close Sidebar(sidebar)(aria-label)': '關閉側邊欄',
      'Collapse Sidebar(sidebar)(aria-label)': '收合側邊欄',
      'Hide Sidebar(sidebar)': '隱藏側邊欄',
      'Show Sidebar(sidebar)': '顯示側邊欄',
      'Copy Text(code block)(aria-label)': '複製程式碼',
      'Copied Text(code block)(aria-label)': '已複製',
    },
    ja: {
      displayName: '日本語',
      'Search(search dialog)': '検索',
      'Search(search trigger)': 'ドキュメントを検索',
      'No results found(search dialog)': '結果が見つかりません',
      'Open Search(search trigger)(aria-label)': '検索を開く',
      'Close Search(search dialog)(aria-label)': '検索を閉じる',
      'On this page(table of contents)': 'このページの内容',
      'No Headings(table of contents)': '見出しなし',
      'Table of Contents(inline table of contents)': '目次',
      'Next Page(pagination)': '次のページ',
      'Previous Page(pagination)': '前のページ',
      'Last updated on(page footer)': '最終更新日',
      'Edit on GitHub(edit page)': 'GitHub で編集',
      'Choose a language(language switcher)': '言語を選択',
      'Choose a language(language switcher)(aria-label)': '言語を選択',
      'Toggle Theme(theme switcher)(aria-label)': 'テーマを切り替え',
      'Light(theme switcher)(aria-label)': 'ライト',
      'Dark(theme switcher)(aria-label)': 'ダーク',
      'System(theme switcher)(aria-label)': 'システム',
      'Page Not Found(404 not found page)': 'ページが見つかりません',
      'Back to Home(404 not found page)': 'ホームに戻る',
      'The page you are looking for might have been removed, had its name changed, or is temporarily unavailable.(404 not found page)':
        'お探しのページは削除されたか、名前が変更されたか、一時的に利用できない可能性があります。',
      'Copy Markdown(page actions)': 'Markdown をコピー',
      'View as Markdown(page actions)': 'Markdown で表示',
      'Open in GitHub(page actions)': 'GitHub で開く',
      'Open(page actions)': '開く',
      'Toggle Menu(home layout header)(aria-label)': 'メニューを切り替え',
      'Open Sidebar(aria-label)': 'サイドバーを開く',
      'Open Sidebar(sidebar)(aria-label)': 'サイドバーを開く',
      'Close Sidebar(aria-label)': 'サイドバーを閉じる',
      'Close Sidebar(sidebar)(aria-label)': 'サイドバーを閉じる',
      'Collapse Sidebar(sidebar)(aria-label)': 'サイドバーを折りたたむ',
      'Hide Sidebar(sidebar)': 'サイドバーを隠す',
      'Show Sidebar(sidebar)': 'サイドバーを表示',
      'Copy Text(code block)(aria-label)': 'コードをコピー',
      'Copied Text(code block)(aria-label)': 'コピーしました',
    },
  });

const docsLinkLabel: Record<Locale, string> = {
  en: 'Documentation',
  zh: '文档',
  'zh-TW': '文件',
  ja: 'ドキュメント',
};

export function baseOptions(locale: string): BaseLayoutProps {
  const label = docsLinkLabel[locale as Locale] ?? docsLinkLabel.en;

  return {
    nav: {
      title: (
        <>
          <Image src="/monoize.svg" alt="" width={22} height={22} />
          <span className="font-display font-semibold">{appName}</span>
        </>
      ),
      url: `/${locale}`,
    },
    links: [
      {
        text: label,
        url: `/${locale}/docs`,
        active: 'nested-url',
      },
    ],
    githubUrl: `https://github.com/${gitConfig.user}/${gitConfig.repo}`,
  };
}
