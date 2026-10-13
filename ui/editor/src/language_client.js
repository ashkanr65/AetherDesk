export function installLanguageClient(monaco, send, resolveId) {
  let sequence = 0;
  const pending = new Map(), languages = new Map();
  function request(method, params, token) {
    if (pending.size >= 64) return Promise.reject(Error('Too many pending language requests'));
    if (token?.isCancellationRequested) return Promise.resolve(null);
    return new Promise((resolve, reject) => {
      const requestId = ++sequence;
      const item = {resolve, reject, timer:null, cancellation:null};
      const remove = () => { pending.delete(requestId); clearTimeout(item.timer); item.cancellation?.dispose(); };
      item.timer = setTimeout(() => { remove(); reject(Error('Language server timed out')); }, 25000);
      pending.set(requestId, item);
      item.cancellation = token?.onCancellationRequested(() => { remove(); resolve(null); send('languageCancel', {requestId}); });
      send('languageRequest', {requestId, method, params});
    });
  }
  const position = p => ({line:p.lineNumber-1, character:p.column-1});
  const range = r => ({startLineNumber:r.start.line+1,startColumn:r.start.character+1,endLineNumber:r.end.line+1,endColumn:r.end.character+1});
  const document = model => ({uri:model.uri.toString()});
  const versions = () => new Map(monaco.editor.getModels().map(model => [model.uri.toString(), model.getVersionId()]));
  function deactivate() {
    for (const registrations of languages.values()) for (const registration of registrations) registration.dispose();
    languages.clear();
    for (const model of monaco.editor.getModels()) monaco.editor.setModelMarkers(model, 'host-language', []);
    for (const item of pending.values()) { clearTimeout(item.timer); item.cancellation?.dispose(); item.resolve(null); }
    pending.clear();
  }
  function activate(language) {
    if (languages.has(language)) return;
    const registrations=[]; languages.set(language, registrations);
    const kindNames=['Text','Method','Function','Constructor','Field','Variable','Class','Interface','Module','Property','Unit','Value','Enum','Keyword','Snippet','Color','File','Reference','Folder','EnumMember','Constant','Struct','Event','Operator','TypeParameter'];
    registrations.push(monaco.languages.registerCompletionItemProvider(language, {
      triggerCharacters:['.','>','/',':'],
      async provideCompletionItems(model,pos,context,token) {
        const version=model.getVersionId();
        const result=await request('textDocument/completion',{textDocument:document(model),position:position(pos),context:{triggerKind:context.triggerKind,triggerCharacter:context.triggerCharacter}},token);
        if(model.isDisposed()||model.getVersionId()!==version||token?.isCancellationRequested)return {suggestions:[]};
        const word=model.getWordUntilPosition(pos), fallback={startLineNumber:pos.lineNumber,endLineNumber:pos.lineNumber,startColumn:word.startColumn,endColumn:word.endColumn};
        return {suggestions:(Array.isArray(result)?result:result?.items??[]).map(item=>({label:item.label,detail:item.detail,documentation:typeof item.documentation==='string'?item.documentation:{value:item.documentation?.value??'',isTrusted:false},kind:monaco.languages.CompletionItemKind[kindNames[(item.kind??1)-1]??'Text'],insertText:item.textEdit?.newText??item.insertText??item.label,insertTextRules:item.insertTextFormat===2?monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet:undefined,range:item.textEdit?.range?range(item.textEdit.range):item.textEdit?.replace?range(item.textEdit.replace):fallback,sortText:item.sortText,filterText:item.filterText})),incomplete:result?.isIncomplete??false};
      }
    }));
    registrations.push(monaco.languages.registerDefinitionProvider(language, {async provideDefinition(model,pos,token) {
      const result=await request('textDocument/definition',{textDocument:document(model),position:position(pos)},token);
      return (Array.isArray(result)?result:result?[result]:[]).map(value=>({uri:monaco.Uri.parse(value.uri??value.targetUri),range:range(value.range??value.targetSelectionRange??value.targetRange)}));
    }}));
    registrations.push(monaco.languages.registerHoverProvider(language, {async provideHover(model,pos,token) {
      const result=await request('textDocument/hover',{textDocument:document(model),position:position(pos)},token); if(!result)return null;
      const contents=Array.isArray(result.contents)?result.contents:[result.contents];
      return {contents:contents.map(value=>({value:typeof value==='string'?value:value?.value??'',isTrusted:false})),range:result.range?range(result.range):undefined};
    }}));
    registrations.push(monaco.languages.registerDocumentFormattingEditProvider(language, {async provideDocumentFormattingEdits(model,options,token) {
      const version=model.getVersionId();
      const result=await request('textDocument/formatting',{textDocument:document(model),options:{tabSize:options.tabSize,insertSpaces:options.insertSpaces}},token);
      if(model.isDisposed()||version!==model.getVersionId()||token?.isCancellationRequested)return [];
      return (result??[]).map(item=>({range:range(item.range),text:item.newText}));
    }}));
    registrations.push(monaco.languages.registerRenameProvider(language, {async provideRenameEdits(model,pos,newName,token) {
      const before=versions();
      const result=await request('textDocument/rename',{textDocument:document(model),position:position(pos),newName},token);
      if(!result)return {edits:[],rejectReason:'The language server could not rename this symbol.'};
      const edits=[], changes={...result.changes};
      for(const change of result.documentChanges??[]) {
        if(!change.textDocument)return {edits:[],rejectReason:'This rename requires filesystem changes. Use a host IDE for this operation.'};
        changes[change.textDocument.uri]=change.edits;
      }
      for(const [uri,items] of Object.entries(changes)) {
        if(!resolveId(uri))return {edits:[],rejectReason:'Open all affected host files before renaming this symbol.'};
        const target=monaco.editor.getModel(monaco.Uri.parse(uri));
        const version=before.get(uri);
        if(!target||version===undefined||target.getVersionId()!==version)return {edits:[],rejectReason:'A document changed while rename was being calculated. Retry the rename.'};
        for(const item of items)edits.push({resource:monaco.Uri.parse(uri),textEdit:{range:range(item.range),text:item.newText},versionId:version});
      }
      return {edits};
    }}));
  }
  function receive(message) {
    if(message.type==='languageResult') {
      const item=pending.get(message.requestId);if(!item)return;pending.delete(message.requestId);clearTimeout(item.timer);item.cancellation?.dispose();
      if(message.error)item.reject(Error(message.error));else item.resolve(message.result);
    }else if(message.type==='languageActivate')activate(message.language);
    else if(message.type==='languageDeactivate')deactivate();
    else if(message.type==='diagnostics') {
      if(!resolveId(message.uri))return;
      const model=monaco.editor.getModel(monaco.Uri.parse(message.uri));if(!model)return;
      monaco.editor.setModelMarkers(model,'host-language',(message.diagnostics??[]).map(d=>({...range(d.range),message:d.message,severity:[0,8,4,2,1][d.severity??1]??8,source:d.source,code:d.code==null?undefined:String(d.code)})));
    }
  }
  return {receive};
}
