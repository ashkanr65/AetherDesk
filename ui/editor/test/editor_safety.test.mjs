import test from 'node:test';
import assert from 'node:assert/strict';
import {applyExactChanges, applyBoundedExactChanges, shouldReplace, validDocument, validDocumentIdentity} from '../src/bridge.js';
import {installLanguageClient} from '../src/language_client.js';

const change=(sl,sc,el,ec,text)=>({range:{startLineNumber:sl,startColumn:sc,endLineNumber:el,endColumn:ec},text});
test('mixed original line endings survive regular edits and joining lines',()=>{
  const original='one\r\ntwo\nthree\rfour';
  const edited=applyExactChanges(original,{changes:[change(2,1,2,4,'TWO')]});
  assert.equal(edited,'one\r\nTWO\nthree\rfour');
  assert.equal(applyExactChanges(edited,{changes:[change(1,4,2,1,'')]}),'oneTWO\nthree\rfour');
  assert.equal(applyExactChanges(original,{changes:[change(1,1,1,4,'ONE'),change(4,1,4,5,'FOUR')]}),'ONE\r\ntwo\nthree\rFOUR');
});
test('exact untouched text is not replaced by its normalized Monaco value',()=>{
  const text='a\r\nb\nc';
  const record={text,generation:2,model:{getValue:()=> 'a\nb\nc'}};
  assert.equal(shouldReplace(record,{text,generation:3}),false);
  assert.equal(applyExactChanges(text,{isEolChange:true,eol:'\n'}),'a\nb\nc');
});
test('eight-megabyte documents remain accepted by the editor protocol',()=>{
  assert(validDocument({id:'large',text:'a'.repeat(8*1024*1024),generation:0}));
});

function fixture(){
  const providers={},messages=[];
  const uri='file:///host/file.cpp';
  const model={uri:{toString:()=>uri},version:1,getVersionId(){return this.version;},isDisposed:()=>false};
  const registrations=[];
  const languages={CompletionItemKind:{Text:18},CompletionItemInsertTextRule:{InsertAsSnippet:4}};
  for(const [name,key] of [['CompletionItem','completion'],['Definition','definition'],['Hover','hover'],['Rename','rename'],['DocumentFormattingEdit','formatting']])languages[`register${name}Provider`]=(_,provider)=>{providers[key]=provider;const registration={disposed:false,dispose(){this.disposed=true;}};registrations.push(registration);return registration;};
  const monaco={languages,Uri:{parse:value=>({toString:()=>value})},editor:{getModels:()=>[model],getModel:value=>value.toString()===uri?model:null,setModelMarkers(){}}};
  const client=installLanguageClient(monaco,(type,message)=>messages.push({type,...message}),value=>value===uri?'document':null);
  client.receive({type:'languageActivate',language:'cpp'});
  const reply=result=>client.receive({type:'languageResult',requestId:messages.at(-1).requestId,result});
  const edit={range:{start:{line:0,character:0},end:{line:0,character:3}},newText:'new'};
  return {client,providers,messages,model,uri,reply,edit,registrations};
}
test('rename rejects responses after an affected document changes',async()=>{
  const f=fixture();const pending=f.providers.rename.provideRenameEdits(f.model,{lineNumber:1,column:1},'new');
  f.model.version++;f.reply({changes:{[f.uri]:[f.edit]}});
  const result=await pending;assert.equal(result.edits.length,0);assert.match(result.rejectReason,/changed/);
});
test('rename carries version guards and rejects unopened files',async()=>{
  const f=fixture();let pending=f.providers.rename.provideRenameEdits(f.model,{lineNumber:1,column:1},'new');
  f.reply({changes:{[f.uri]:[f.edit]}});assert.equal((await pending).edits[0].versionId,1);
  pending=f.providers.rename.provideRenameEdits(f.model,{lineNumber:1,column:1},'new');
  f.reply({changes:{'file:///not-open.cpp':[f.edit]}});assert.equal((await pending).edits.length,0);
});
test('formatting is guarded against edits and disabling support releases providers',async()=>{
  const f=fixture();const pending=f.providers.formatting.provideDocumentFormattingEdits(f.model,{tabSize:4,insertSpaces:true});
  f.model.version++;f.reply([f.edit]);assert.deepEqual(await pending,[]);
  const hover=f.providers.hover.provideHover(f.model,{lineNumber:1,column:1});
  f.client.receive({type:'languageDeactivate'});assert.equal(await hover,null);assert(f.registrations.every(r=>r.disposed));
});

test('oversized changes retain the exact earlier buffer and identity remains valid',()=>{
  const text='a\r\nb';
  const result=applyBoundedExactChanges(text,{changes:[change(2,2,2,2,'x'.repeat(20))]},8);
  assert.equal(result.accepted,false);assert.equal(result.text,text);
  const oversized={id:'keep-open',generation:2,text:'a'.repeat(16*1024*1024+1)};
  assert.equal(validDocument(oversized),false);assert.equal(validDocumentIdentity(oversized),true);
});
