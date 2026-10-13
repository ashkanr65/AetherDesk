export const protocolVersion=1;
export const maxEditorCharacters=16*1024*1024;
export function validDocumentIdentity(value){return value!==null&&typeof value==='object'&&!Array.isArray(value)&&typeof value.id==='string'&&value.id.length>0&&value.id.length<=256;}
export function validMessage(message,token) {
  return message!==null&&typeof message==='object'&&!Array.isArray(message)&&message.version===protocolVersion&&message.token===token&&typeof message.type==='string';
}
export function validDocument(value) {
  return validDocumentIdentity(value)&&typeof value.text==='string'&&value.text.length<=maxEditorCharacters&&Number.isSafeInteger(value.generation)&&value.generation>=0;
}
export function makeBridge(webview,token,onMessage) {
  const send=(type,fields={})=>webview?.postMessage({version:protocolVersion,token,type,...fields});
  webview?.addEventListener('message',event=>{
    if(validMessage(event.data,token))onMessage(event.data);
  });
  return send;
}
// An echoed application snapshot must never replace a newer editor edit.
export function shouldReplace(record,incoming) {
  return incoming.generation>record.generation&&incoming.text!==(record.text??record.model.getValue());
}

// Monaco normalizes model EOLs. Keep the exact on-disk text separately and map
// its edits by line/column so unchanged CRLF, LF and CR separators survive.
export function applyExactChanges(text,event) {
  if(event.isEolChange)return text.replace(/\r\n|\r|\n/g,event.eol);
  const starts=[0];const newline=/\r\n|\r|\n/g;let match;
  while((match=newline.exec(text)))starts.push(match.index+match[0].length);
  const offset=(line,column)=>{
    if(!Number.isSafeInteger(line)||!Number.isSafeInteger(column)||line<1||line>starts.length||column<1)throw Error('Invalid editor change position');
    return starts[line-1]+column-1;
  };
  const changes=event.changes.map(change=>({start:offset(change.range.startLineNumber,change.range.startColumn),end:offset(change.range.endLineNumber,change.range.endColumn),text:change.text})).sort((a,b)=>b.start-a.start||b.end-a.end);
  for(const change of changes){if(change.start>change.end||change.end>text.length)throw Error('Invalid editor change range');text=text.slice(0,change.start)+change.text+text.slice(change.end);}
  return text;
}

export function applyBoundedExactChanges(text,event,limit=maxEditorCharacters){
  const updated=applyExactChanges(text,event);
  return updated.length<=limit?{accepted:true,text:updated}:{accepted:false,text};
}
