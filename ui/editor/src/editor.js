import * as monaco from 'monaco-editor/esm/vs/editor/editor.main.js';
import {makeBridge,validDocument,validDocumentIdentity,shouldReplace,applyBoundedExactChanges} from './bridge.js';
import {installLanguageClient} from './language_client.js';
import './editor.css';
self.MonacoEnvironment={getWorker(_module,label){const worker=label==='json'?'json':label==='css'||label==='scss'||label==='less'?'css':label==='html'||label==='handlebars'||label==='razor'?'html':label==='typescript'||label==='javascript'?'ts':'editor';return new Worker(new URL(`${worker}.worker.js`,location.href));}};
const token=location.hash.substring(1);
const models=new Map();let activeId=null;let suppress=false;let diff=null;let compareModel=null;
const container=document.getElementById('editor'),diffContainer=document.getElementById('diff');
const options={theme:'vs-dark',automaticLayout:true,fontFamily:"Cascadia Code, Consolas, monospace",fontSize:14,tabSize:4,insertSpaces:true,detectIndentation:true,minimap:{enabled:false},scrollBeyondLastLine:false,renderWhitespace:'selection',bracketPairColorization:{enabled:true},guides:{bracketPairs:true},links:false,stickyScroll:{enabled:true},unicodeHighlight:{ambiguousCharacters:false},accessibilitySupport:'auto',quickSuggestions:{other:true,comments:false,strings:false},wordWrap:'off'};
const editor=monaco.editor.create(container,options);
// File contents remain plain model data. No document link can navigate this view.
monaco.editor.registerLinkOpener({open:()=>true});
monaco.editor.registerEditorOpener({openCodeEditor(_source,resource,selection){
  if(selection&&!('startLineNumber' in selection))selection={startLineNumber:selection.lineNumber,startColumn:selection.column,endLineNumber:selection.lineNumber,endColumn:selection.column};
  const record=[...models.entries()].find(([,v])=>v.model.uri.toString()===resource.toString());
  if(record){send('select',{id:record[0]});select(record[0]);if(selection)editor.setSelection(selection);return true;}
  send('openDefinition',{uri:resource.toString(),sourceId:activeId,selection});return true;
}});
document.addEventListener('click',event=>{if(event.target.closest?.('a'))event.preventDefault();},true);
document.addEventListener('dragover',event=>event.preventDefault());document.addEventListener('drop',event=>event.preventDefault());
window.open=()=>null;
const send=makeBridge(window.chrome?.webview,token,receive);
const language=installLanguageClient(monaco,send,uri=>[...models.entries()].find(([,v])=>v.remote&&v.model.uri.toString()===uri)?.[0]);
function snapshot(type,id=activeId,extra={}) {const record=models.get(id);if(record)send(type,{id,text:record.text,generation:record.generation,...extra});}
function select(id) {
  if(!models.has(id))return;
  if(activeId&&models.has(activeId))models.get(activeId).view=editor.saveViewState();
  endCompare();activeId=id;const record=models.get(id);editor.setModel(record.model);if(record.view)editor.restoreViewState(record.view);editor.focus();
}
function endCompare(){if(diff){diff.dispose();diff=null;}compareModel?.dispose();compareModel=null;diffContainer.hidden=true;container.hidden=false;}
function sync(message,partial=false){
  if(!Array.isArray(message.documents))return;
  const valid=message.documents.filter(validDocument);const ids=new Set(message.documents.filter(validDocumentIdentity).map(v=>v.id));
  for(const [id,record] of models)if(!partial&&!ids.has(id)){record.listener.dispose();record.model.dispose();models.delete(id);}
  for(const doc of valid){
    let record=models.get(doc.id);
    if(!record){
      let uri;try{uri=monaco.Uri.parse(doc.uri);}catch{uri=monaco.Uri.from({scheme:'aetherdesk',path:`/${encodeURIComponent(doc.id)}/${encodeURIComponent(doc.name??'file.txt')}`});}
      if(monaco.editor.getModel(uri))uri=monaco.Uri.from({scheme:'aetherdesk',path:`/${encodeURIComponent(doc.id)}/${encodeURIComponent(doc.name??'file.txt')}`});
      const model=monaco.editor.createModel(doc.text,doc.language??'plaintext',uri);record={model,text:doc.text,remote:doc.remote===true,generation:doc.generation,view:null,listener:null};models.set(doc.id,record);
      record.listener=model.onDidChangeContent(event=>{if(suppress)return;const update=applyBoundedExactChanges(record.text,event);if(!update.accepted){suppress=true;try{model.setValue(record.text);}finally{suppress=false;}send('editorNotice',{id:doc.id,message:'This change exceeds the editor limit of 16 million characters and was not applied. Your earlier edits are preserved.'});return;}record.text=update.text;record.generation++;snapshot('change',doc.id);});
    }else{
      if(shouldReplace(record,doc)){suppress=true;record.model.setValue(doc.text);record.text=doc.text;suppress=false;record.generation=doc.generation;}
      else if(record.text===doc.text)record.generation=Math.max(record.generation,doc.generation);
      if(doc.language&&record.model.getLanguageId()!==doc.language)monaco.editor.setModelLanguage(record.model,doc.language);
    }
  }
  if(message.selectedId&&message.selectedId!==activeId)select(message.selectedId);
  else if(!models.has(activeId)){activeId=null;editor.setModel(null);endCompare();}
}
function receive(message){
  switch(message.type){
    case 'sync':sync(message);break;
    case 'syncDocument':sync({documents:[message.document]},true);break;
    case 'syncSelection':{const ids=new Set(message.ids??[]);for(const [id,record] of models)if(!ids.has(id)){record.listener.dispose();record.model.dispose();models.delete(id);}if(message.selectedId!==activeId)select(message.selectedId);if(!models.has(activeId)){activeId=null;editor.setModel(null);endCompare();}break;}
    case 'navigate':if(message.id===activeId&&message.selection){editor.setSelection(message.selection);editor.revealRangeInCenter(message.selection);}break;
    case 'flush':snapshot('flushed',message.id,{requestId:message.requestId});break;
    case 'focus':editor.focus();break;
    case 'command':if(['actions.find','editor.action.startFindReplaceAction','undo','redo','editor.action.formatDocument','editor.action.quickCommand','editor.action.rename','editor.action.revealDefinition'].includes(message.command))editor.trigger('toolbar',message.command,{});break;
    case 'preferences':editor.updateOptions(message.options??{});break;
    case 'language':{const item=models.get(message.id);if(item)monaco.editor.setModelLanguage(item.model,message.language);break;}
    case 'compare':{const item=models.get(message.id);if(!item||typeof message.text!=='string')return;endCompare();container.hidden=true;diffContainer.hidden=false;compareModel=monaco.editor.createModel(message.text,item.model.getLanguageId());diff=monaco.editor.createDiffEditor(diffContainer,{...options,readOnly:true,originalEditable:false,renderSideBySide:true});diff.setModel({original:compareModel,modified:item.model});break;}
    case 'endCompare':endCompare();break;
    default:language.receive(message);
  }
}
editor.addCommand(monaco.KeyMod.CtrlCmd|monaco.KeyCode.KeyS,()=>snapshot('save'));
editor.addCommand(monaco.KeyMod.CtrlCmd|monaco.KeyCode.KeyW,()=>snapshot('close'));
send('ready');
