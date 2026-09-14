const overview = [
 ['', 'Use `image_resolve_urls` in the response phase to return upstream image URLs as `b64_json` through the Images API.'],
 ['.zh', '在响应阶段启用 `image_resolve_urls`，可将上游图片 URL 转为 Images API 的 `b64_json` 返回给下游。'],
 ['.zh-TW', '在回應階段啟用 `image_resolve_urls`，可將上游圖片 URL 轉為 Images API 的 `b64_json` 傳回下游。'],
 ['.ja', 'レスポンスフェーズで `image_resolve_urls` を有効にすると、上流の画像 URL を Images API の `b64_json` として下流に返します。']
];
for(const [suffix,text] of overview) {const path='docs/content/docs/transforms/index'+suffix+'.mdx'; let s=await Bun.file(path).text(); const at=s.indexOf('\n## ',s.indexOf('| Image |')>=0?s.indexOf('| Image |'):s.indexOf('| `image')>=0?s.indexOf('| `image'):s.indexOf('image_compress_input')); s=s.slice(0,at)+'\n'+text+'\n'+s.slice(at);await Bun.write(path,s);}
