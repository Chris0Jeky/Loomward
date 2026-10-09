"""Narrow dual-era MCP stdio reference. Not a general SDK or a certified server.

Implements tools, two fixed resources, ping and discovery/legacy lifecycle.
Serial bounded requests only; no HTTP, subscriptions, elicitation, tasks,
roots, sampling or external process launch. See docs/28 for the support matrix.
"""
from __future__ import annotations
from collections import deque
import json
import sys
import time
from typing import BinaryIO
from .catalog import QueryBudgetExceeded
from .interop import ToolService, SCHEMAS
from .teacher import strict_json

MODERN = '2026-07-28'
LEGACY = '2025-11-25'
SUPPORTED = [MODERN, LEGACY]
MAX_FRAME = 65_536
MAX_OUTPUT = 262_144
INFO = {'name':'loomward-reference','version':'0.2.0'}
CAPABILITIES = {'tools':{},'resources':{}}
PREFIX = 'io.modelcontextprotocol/'


def error(code, message, ident=None, data=None):
    result = {'jsonrpc':'2.0','error':{'code':code,'message':message}}
    if type(ident) is int or isinstance(ident,str): result['id']=ident
    if data is not None: result['error']['data']=data
    return result


def _params(params, allowed, required=()):
    if set(params) - (set(allowed) | {'_meta'}) or set(required) - set(params):
        raise ValueError('Invalid method parameters')


class Protocol:
    def __init__(self, service: ToolService):
        self.service = service
        self.legacy_phase = 'new'
        self._tool_calls = deque()

    def _result(self, ident, result, modern):
        if modern:
            result = dict(result, resultType='complete', _meta={PREFIX+'serverInfo':dict(INFO)})
        return {'jsonrpc':'2.0','id':ident,'result':result}

    def handle(self, message):
        if not isinstance(message,dict) or message.get('jsonrpc') != '2.0' or not isinstance(message.get('method'),str):
            return error(-32600,'Invalid JSON-RPC request')
        ident = message.get('id')
        if 'id' not in message:
            if message['method']=='notifications/initialized' and self.legacy_phase=='initializing':
                self.legacy_phase='ready'
            return None  # Never execute request methods or reply to notifications.
        if not (type(ident) is int and abs(ident)<=9007199254740991 or isinstance(ident,str) and 0<len(ident)<=128):
            return error(-32600,'Invalid request ID')
        params=message.get('params',{})
        if not isinstance(params,dict): return error(-32602,'Parameters must be an object',ident)
        method=message['method']
        meta=params.get('_meta',{})
        if not isinstance(meta,dict): return error(-32602,'Invalid request metadata',ident)
        modern=method=='server/discover' or any(k.startswith(PREFIX) for k in meta)
        if modern:
            version=meta.get(PREFIX+'protocolVersion')
            if not isinstance(version,str) or not isinstance(meta.get(PREFIX+'clientCapabilities'),dict):
                return error(-32602,'Required per-request protocol version and client capabilities are missing',ident)
            if version!=MODERN:
                return error(-32022,'Unsupported protocol version',ident,{'supported':[MODERN],'requested':version})
        elif method!='initialize' and self.legacy_phase!='ready':
            return error(-32602,'Use modern per-request metadata or complete legacy initialization',ident)
        try:
            if method=='initialize' and not modern:
                _params(params,{'protocolVersion','capabilities','clientInfo'},{'protocolVersion','capabilities','clientInfo'})
                if self.legacy_phase!='new': raise ValueError('Legacy initialization already performed')
                if not isinstance(params['protocolVersion'],str) or not isinstance(params['capabilities'],dict):
                    raise ValueError('Invalid legacy initialization')
                info=params['clientInfo']
                if not isinstance(info,dict) or not all(isinstance(info.get(k),str) and 0<len(info[k])<=128 for k in ('name','version')):
                    raise ValueError('Invalid legacy client information')
                self.legacy_phase='initializing'
                return self._result(ident,{'protocolVersion':LEGACY,'capabilities':dict(CAPABILITIES),'serverInfo':dict(INFO),
                                          'instructions':'Snapshot-only reference. No approve, execute, content-read or scope-expansion capability.'},False)
            if method=='server/discover':
                _params(params,set())
                data={'supportedVersions':SUPPORTED,'capabilities':dict(CAPABILITIES),
                      'instructions':'Read scoped metadata and simulate supplied placement. No filesystem or process control.'}
            elif method=='ping':
                _params(params,set());data={}
            elif method=='tools/list':
                _params(params,{'cursor'})
                if 'cursor' in params: raise ValueError('Tool list is not paginated')
                data={'tools':self.service.tool_specs()}
            elif method=='tools/call':
                _params(params,{'name','arguments'},{'name'})
                name=params['name'];arguments=params.get('arguments',{})
                if not isinstance(name,str) or name not in SCHEMAS:
                    return error(-32602,'Unknown tool',ident)
                if not isinstance(arguments,dict): raise ValueError('Tool arguments must be an object')
                now=time.monotonic()
                while self._tool_calls and now-self._tool_calls[0]>=60: self._tool_calls.popleft()
                try:
                    if len(self._tool_calls)>=120: raise ValueError('Reference tool-call budget exceeded; retry later')
                    self._tool_calls.append(now)
                    value=self.service.call(name,arguments)
                    data={'content':[{'type':'text','text':json.dumps(value,ensure_ascii=True,allow_nan=False,separators=(',',':'))}],
                          'structuredContent':value,'isError':False}
                except (ValueError,PermissionError,LookupError,QueryBudgetExceeded) as exc:
                    data={'content':[{'type':'text','text':str(exc)[:512]}],'isError':True}
            elif method=='resources/list':
                _params(params,{'cursor'})
                if 'cursor' in params: raise ValueError('Resource list is not paginated')
                data={'resources':self.service.resources()}
            elif method=='resources/read':
                _params(params,{'uri'},{'uri'})
                if not isinstance(params['uri'],str): raise ValueError('Resource URI must be a string')
                value=self.service.resource(params['uri'])
                data={'contents':[{'uri':params['uri'],'mimeType':'application/json',
                                   'text':json.dumps(value,ensure_ascii=True,allow_nan=False)}]}
            else:
                return error(-32601,'Method not implemented by this bounded reference',ident)
            return self._result(ident,data,modern)
        except (ValueError,LookupError) as exc:
            return error(-32602,str(exc)[:512],ident)
        except Exception:
            return error(-32603,'Internal reference error; no action was executed',ident)


def _depth_ok(value, limit=32):
    stack=[(value,0)]
    while stack:
        node,depth=stack.pop()
        if depth>limit: return False
        if isinstance(node,dict):stack.extend((v,depth+1) for v in node.values())
        elif isinstance(node,list):stack.extend((v,depth+1) for v in node)
    return True


def serve(protocol: Protocol, source: BinaryIO, sink: BinaryIO) -> None:
    while True:
        raw=source.readline(MAX_FRAME+1)
        if not raw: return
        if len(raw)>MAX_FRAME:
            response=error(-32600,'Request frame exceeds 65536 bytes')
            drained=len(raw)
            while not raw.endswith(b'\n'):
                raw=source.readline(MAX_FRAME+1);drained+=len(raw)
                if not raw: break
                if drained>1_048_576:
                    sink.write(json.dumps(response).encode()+b'\n');sink.flush();return
        else:
            try:
                message=strict_json(raw.decode('utf-8'))
                if not _depth_ok(message): raise ValueError('Depth limit')
                response=protocol.handle(message)
            except (ValueError,UnicodeError,RecursionError):
                response=error(-32700,'Malformed, duplicate-key, nonfinite or excessively nested JSON')
        if response is not None:
            encoded=json.dumps(response,ensure_ascii=True,allow_nan=False,separators=(',',':')).encode()
            if len(encoded)>MAX_OUTPUT:
                encoded=json.dumps(error(-32603,'Reference response size budget exceeded',response.get('id'))).encode()
            sink.write(encoded+b'\n');sink.flush()
