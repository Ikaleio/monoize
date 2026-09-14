const base = 'docs/content/docs/transforms/image_resolve_urls';
const variants = [
 ['', 'request image URLs', 'request or response image URLs', 'image URLs in the request', 'image URLs in requests or responses', 'Return base64 images to downstream clients', 'For an Images API client that requires `b64_json`, apply the transform in the response phase. Monoize downloads the upstream URL and encodes the image as `data[].b64_json`.'],
 ['.zh', '请求中的图片 URL', '请求或响应中的图片 URL', '请求中的图片 URL', '请求或响应中的图片 URL', '向下游返回 base64 图片', '如果 Images API 客户端需要 `b64_json`，请在响应阶段启用此变换。Monoize 下载上游图片 URL，并将图片编码为 `data[].b64_json`。'],
 ['.zh-TW', '請求中的圖片 URL', '請求或回應中的圖片 URL', '請求中的圖片 URL', '請求或回應中的圖片 URL', '向下游傳回 base64 圖片', '若 Images API 用戶端需要 `b64_json`，請在回應階段啟用此變換。Monoize 下載上游圖片 URL，並將圖片編碼為 `data[].b64_json`。'],
 ['.ja', 'リクエスト内の画像 URL', 'リクエストまたはレスポンス内の画像 URL', 'リクエスト内の画像 URL', 'リクエストまたはレスポンス内の画像 URL', '下流に base64 画像を返す', 'Images API クライアントが `b64_json` を必要とする場合は、レスポンスフェーズでこの変換を有効にします。Monoize は上流の画像 URL をダウンロードし、`data[].b64_json` として返します。']
];
const rule = {transform:'image_resolve_urls',enabled:true,phase:'response',models:['gpt-image-2.5-sunburst'],config:{timeout_seconds:30,max_bytes:20971520,roles:['assistant']}};
for (const [suffix,from,to,from2,to2,title,desc] of variants) {
 const path=base+suffix+'.mdx'; let s=await Bun.file(path).text();
 s=s.replaceAll(from,to); if (from2!==from) s=s.replaceAll(from2,to2);
 s=s.replace('| `request` |','| `request`, `response` |');
 s+='\n### '+title+'\n\n'+desc+'\n\n```json\n'+JSON.stringify(rule,null,2)+'\n```\n'; await Bun.write(path,s);
}
for (const path of ['Cargo.toml','Cargo.lock']) {let s=await Bun.file(path).text(); s=path==='Cargo.toml'?s.replace('version = "1.9.5"','version = "1.9.6"'):s.replace('name = "monoize"\nversion = "1.9.5"','name = "monoize"\nversion = "1.9.6"'); await Bun.write(path,s);}
const overview = [
 ['', 'Use `image_resolve_urls` in the response phase to return upstream image URLs as `b64_json` through the Images API.'],
 ['.zh', '在响应阶段启用 `image_resolve_urls`，可将上游图片 URL 转为 Images API 的 `b64_json` 返回给下游。'],
 ['.zh-TW', '在回應階段啟用 `image_resolve_urls`，可將上游圖片 URL 轉為 Images API 的 `b64_json` 傳回下游。'],
 ['.ja', 'レスポンスフェーズで `image_resolve_urls` を有効にすると、上流の画像 URL を Images API の `b64_json` として下流に返します。']
];
for(const [suffix,text] of overview) {const path='docs/content/docs/transforms/index'+suffix+'.mdx'; let s=await Bun.file(path).text(); const at=s.indexOf('\n## ',s.indexOf('| Image |')>=0?s.indexOf('| Image |'):s.indexOf('| `image')>=0?s.indexOf('| `image'):s.indexOf('image_compress_input')); s=s.slice(0,at)+'\n'+text+'\n'+s.slice(at);await Bun.write(path,s);}
