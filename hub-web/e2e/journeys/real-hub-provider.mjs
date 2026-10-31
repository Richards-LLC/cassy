// Inert supervisor used only by the native real-hub launch journey.
export function realHubProviderScript(cli) {
  return `#!/usr/bin/python3
import sys,os,json,subprocess,uuid,tty
from datetime import datetime,timezone
if sys.argv[1:3] == ['auth','status']: print('{"loggedIn":true}');sys.exit(0)
if '--version' in sys.argv: print('2.1.47');sys.exit(0)
if '--help' in sys.argv: print('--dangerously-skip-permissions --model --agent --mcp-config');sys.exit(0)
# Real harnesses consume paste frames without canonical line buffering. A PTY need not map
# the submit CR to LF, and canonical buffers can truncate a startup prompt.
if sys.stdin.isatty(): tty.setcbreak(sys.stdin.fileno())
sid=sys.argv[sys.argv.index('--session-id')+1] if '--session-id' in sys.argv else str(uuid.uuid4())
transcript=os.path.join(os.environ['HOME'],'.claude','projects',os.getcwd().replace('/','-').replace('.','-'),sid+'.jsonl')
os.makedirs(os.path.dirname(transcript),exist_ok=True)
open(transcript,'a').close()
def hook(event):
 payload=json.dumps(dict(session_id=sid,cwd=os.getcwd(),hook_event_name=event,transcript_path=transcript))
 subprocess.run([${JSON.stringify(cli)},'hook',event],input=payload,text=True,stdout=subprocess.DEVNULL,check=True)
hook('SessionStart')
def reply(message):
 with open(transcript,'a') as f: f.write(json.dumps(dict(type='system',subtype='turn_duration',sessionId=sid,timestamp=datetime.now(timezone.utc).isoformat()))+'\\n')
 payload=json.dumps(dict(schema_version=2,reply_to=None,message=message,summary=message,device_id='*',kind='status',attachments=[]))
 subprocess.run([${JSON.stringify(cli)},'--json','factory','message','--session',os.environ['CAS_FACTORY_SESSION'],'--project-dir',os.getcwd(),'--target','operator','--from','supervisor','--no-wrap','--message',payload],stdout=subprocess.DEVNULL,check=True)
 hook('Stop')
 print('❯ ',flush=True)
print('Claude Code disposable test supervisor\\n❯ ',flush=True)
def prompts():
 pending=b'';message=bytearray();pasting=False
 markers=(b'\\x1b[200~',b'\\x1b[201~')
 while True:
  chunk=os.read(sys.stdin.fileno(),4096)
  if not chunk: return
  pending+=chunk
  while pending:
   if pending.startswith(markers[0]):
    pasting=True;pending=pending[len(markers[0]):]
   elif pending.startswith(markers[1]):
    pasting=False;pending=pending[len(markers[1]):]
   elif any(marker.startswith(pending) for marker in markers): break
   else:
    char=pending[:1];pending=pending[1:]
    if char in (b'\\r',b'\\n') and not pasting:
     yield message.decode('utf8',errors='replace').strip();message.clear()
    elif char != b'\\x1b': message.extend(char)
started=False
for line in prompts():
 if line == '/exit': sys.exit(0)
 if not started:
  reply('Supervisor ready');started=True
 if 'ping launch supervisor' in line: reply('Launch supervisor answered')
`;
}
