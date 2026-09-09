#!/usr/bin/env python3
"""Live phase 1 checks; only touches uniquely named test resources."""
import copy
import json
from pathlib import Path
import subprocess
import uuid

ROOT = Path(__file__).resolve().parents[1]
PREFIX = 'phase1-' + uuid.uuid4().hex[:10]
NS = 'suntitle'
GROUP = 'control.suntitle.io'
HOSTS = f'suntitlehosts.{GROUP}'
NODES = f'suntitlenodes.{GROUP}'
created = []


def kubectl(user, *args, obj=None, fails=None):
    result = subprocess.run(
        ['kubectl', '--kubeconfig', str(ROOT / f'pki/{user}.kubeconfig'),
         '--request-timeout=15s', '-n', NS, *args],
        input=json.dumps(obj) if obj is not None else None,
        text=True, capture_output=True, timeout=25,
    )
    if fails:
        assert result.returncode != 0 and fails in result.stderr, result.stderr
        return
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout) if result.stdout.lstrip().startswith(('{', '[')) else None


def create(user, kind, name, spec, metadata=None):
    obj = {'apiVersion': f'{GROUP}/v1alpha1', 'kind': kind,
           'metadata': {'name': name, 'namespace': NS, **(metadata or {})}, 'spec': spec}
    result = kubectl(user, 'create', '-f', '-', '-o', 'json', obj=obj)
    created.append((HOSTS if kind == 'SuntitleHost' else NODES, name))
    return result


def replace_spec(node, mutate, fails=None):
    obj = copy.deepcopy(node)
    mutate(obj['spec'])
    return kubectl('suntitle-api', 'replace', '-f', '-', '-o', 'json', obj=obj, fails=fails)


def main():
    host_name = PREFIX + '-host'
    host = create('suntitle-proxy', 'SuntitleHost', host_name, {'name': 'Phase 1 host'},
                  {'finalizers': [f'{GROUP}/host-cleanup']})
    owner = {'apiVersion': f'{GROUP}/v1alpha1', 'kind': 'SuntitleHost', 'name': host_name,
             'uid': host['metadata']['uid'], 'controller': True}
    variants = {
        'audio_source': {'sampleRateHz': 16000, 'channels': 1, 'encoding': 'pcm_s16le', 'deviceName': None},
        'transcriber': {'model': 'small', 'language': 'en', 'task': 'transcribe', 'beamSize': 5, 'vadEnabled': True},
        'translator': {'sourceLanguage': 'en', 'targetLanguage': 'cs', 'provider': 'huggingface', 'model': None, 'preserveFormatting': True},
        'display': {'maxLines': 3, 'fontScale': 1.0, 'showOriginal': True, 'theme': 'dark'},
    }
    nodes = {}
    for kind, settings in variants.items():
        nodes[kind] = create('suntitle-api', 'SuntitleNode', PREFIX + '-' + kind.replace('_', '-'),
            {'name': kind, 'hostRef': {'name': host_name}, 'settings': {kind: settings}, 'inputRef': None},
            {'ownerReferences': [owner]})
        nodes[kind] = kubectl('suntitle-proxy', 'patch', NODES, nodes[kind]['metadata']['name'],
            '--type=merge', '-p', json.dumps({'metadata': {'finalizers': [f'{GROUP}/runtime-cleanup']}}), '-o', 'json')
        assert nodes[kind]['spec']['enabled'] is True
    print('PASS all four settings types, default enabled, API owner admission')

    # Proxy also needs owner-reference admission permissions.
    proxy_node = create('suntitle-proxy', 'SuntitleNode', PREFIX + '-proxy', nodes['display']['spec'],
                        {'ownerReferences': [{**owner, 'blockOwnerDeletion': True}]})
    assert proxy_node['metadata']['ownerReferences'][0]['blockOwnerDeletion']

    invalid = [
        ('audio_source', 'channels', 3), ('audio_source', 'encoding', 'opus'),
        ('audio_source', 'sampleRateHz', 12345), ('transcriber', 'model', 'unknown'),
        ('transcriber', 'model', None), ('transcriber', 'language', 'en-US'),
        ('transcriber', 'beamSize', 0), ('translator', 'provider', 'openai'),
        ('translator', 'targetLanguage', 'EN'), ('display', 'fontScale', 0),
        ('display', 'maxLines', 11), ('display', 'theme', 'neon'),
    ]
    for kind, field, value in invalid:
        try:
            replace_spec(nodes[kind], lambda spec: spec['settings'][kind].__setitem__(field, value), fails='Invalid')
        except AssertionError as error:
            raise AssertionError(f'{kind}.{field}={value!r}: {error}') from error
    replace_spec(nodes['display'], lambda spec: spec.__setitem__('settings', {'translator': variants['translator']}), fails='immutable')
    replace_spec(nodes['display'], lambda spec: spec['hostRef'].__setitem__('name', 'other'), fails='immutable')
    for settings in [{}, {'audio_source': variants['audio_source'], 'display': variants['display']},
                     {'transcriber': None}]:
        replace_spec(nodes['display'], lambda spec: spec.__setitem__('settings', settings), fails='Invalid')
    replace_spec(nodes['display'], lambda spec: spec.__setitem__('settings', {'transcriber': variants['audio_source']}), fails='unknown field')
    replace_spec(nodes['audio_source'],
                 lambda spec: spec['settings']['audio_source'].__setitem__('beamSize', 5),
                 fails='unknown field')
    editable = replace_spec(nodes['audio_source'],
                            lambda spec: spec['settings']['audio_source'].__setitem__('channels', 2))
    assert editable['spec']['settings']['audio_source']['channels'] == 2
    minimal = create('suntitle-api', 'SuntitleNode', PREFIX + '-defaults',
                     {'name': 'Defaults', 'hostRef': {'name': host_name},
                      'settings': {'transcriber': {'model': 'small', 'language': 'en'}}})
    assert minimal['spec']['settings'] == {'transcriber': variants['transcriber']}
    print('PASS invalid settings, variant selection, defaults and immutable fields')

    node = nodes['display']
    name = node['metadata']['name']
    old_version = node['metadata']['resourceVersion']
    status = {'phase': 'Running', 'observedGeneration': node['metadata']['generation'],
              'reportingSession': 'phase1-session', 'conditions': [{
                  'type': 'Updated', 'status': 'True', 'observedGeneration': 1,
                  'lastTransitionTime': '2026-09-08T12:00:00Z',
                  'reason': 'DesiredStateApplied', 'message': 'Test acknowledgement'}]}
    kubectl('suntitle-api', 'patch', NODES, name, '--subresource=status', '--type=merge',
            '-p', json.dumps({'status': status}), '-o', 'json', fails='Forbidden')
    updated = kubectl('suntitle-proxy', 'patch', NODES, name, '--subresource=status', '--type=merge',
                      '-p', json.dumps({'status': status}), '-o', 'json')
    assert updated['spec'] == node['spec']
    assert updated['metadata']['generation'] == node['metadata']['generation']
    assert updated['metadata']['resourceVersion'] != old_version
    replace_spec(node, lambda spec: spec.__setitem__('name', 'Stale'), fails='Conflict')
    updated = replace_spec(updated, lambda spec: spec.__setitem__('name', 'Fresh'))
    assert updated['status'] == status
    assert updated['metadata']['generation'] == node['metadata']['generation'] + 1
    # Main endpoint cannot write status even for an otherwise authorized writer.
    result = kubectl('suntitle-proxy', 'patch', NODES, name, '--type=merge',
                     '-p', '{"status":{"phase":"Failed"}}', '-o', 'json')
    assert result['status'] == status
    host_status = {'presence': 'Online', 'sessionId': 'phase1-session',
                   'reportedMetadata': {'nested': {'values': [1, True, 'agent']}}}
    host_updated = kubectl('suntitle-proxy', 'patch', HOSTS, host_name, '--subresource=status',
        '--type=merge', '-p', json.dumps({'status': host_status}), '-o', 'json')
    assert host_updated['spec'] == host['spec']
    assert host_updated['metadata']['generation'] == host['metadata']['generation']
    assert host_updated['status'] == host_status
    assert any(x['metadata']['uid'] == host['metadata']['uid'] for x in
               kubectl('suntitle-api', 'get', HOSTS, '-o', 'json')['items'])
    print('PASS status isolation, preserved generation, optimistic conflict and spec update')

    listing = kubectl('suntitle-proxy', 'get', NODES, '-o', 'json')
    version = listing['metadata']['resourceVersion']
    updated = replace_spec(updated, lambda spec: spec.__setitem__('description', 'watch event'))
    url = f'/apis/{GROUP}/v1alpha1/namespaces/{NS}/suntitlenodes'
    events = subprocess.run(['kubectl', '--kubeconfig', str(ROOT / 'pki/suntitle-proxy.kubeconfig'),
        'get', '--raw', f'{url}?watch=true&resourceVersion={version}&timeoutSeconds=2'],
        text=True, capture_output=True, timeout=15)
    assert events.returncode == 0, events.stderr
    assert any(json.loads(line).get('object', {}).get('metadata', {}).get('resourceVersion') ==
               updated['metadata']['resourceVersion'] for line in events.stdout.splitlines()), events.stdout
    # Reconnect from the last observed cursor, including an event during disconnect.
    version = updated['metadata']['resourceVersion']
    updated = replace_spec(updated, lambda spec: spec.__setitem__('description', 'reconnect event'))
    events = subprocess.run(['kubectl', '--kubeconfig', str(ROOT / 'pki/suntitle-proxy.kubeconfig'),
        'get', '--raw', f'{url}?watch=true&resourceVersion={version}&timeoutSeconds=2'],
        text=True, capture_output=True, timeout=15)
    assert events.returncode == 0, events.stderr
    assert any(json.loads(line).get('object', {}).get('metadata', {}).get('resourceVersion') ==
               updated['metadata']['resourceVersion'] for line in events.stdout.splitlines())
    expired = subprocess.run(['kubectl', '--kubeconfig', str(ROOT / 'pki/suntitle-proxy.kubeconfig'),
        'get', '--raw', f'{url}?watch=true&resourceVersion=1&timeoutSeconds=2'],
        text=True, capture_output=True, timeout=15)
    assert expired.returncode == 0, expired.stderr
    assert any(json.loads(line).get('type') == 'ERROR' and
               json.loads(line).get('object', {}).get('code') == 410
               for line in expired.stdout.splitlines()), 'Expected expired cursor; rerun after Kine compacts history'
    relisted = kubectl('suntitle-proxy', 'get', NODES, '-o', 'json')
    assert any(x['metadata']['uid'] == updated['metadata']['uid'] and
               x['spec']['description'] == 'reconnect event' for x in relisted['items'])
    print('PASS LIST/WATCH replay, reconnect, expired cursor 410 and full relist')

    kubectl('suntitle-api', 'delete', NODES, name, '--wait=false', '-o', 'name')
    terminating = kubectl('suntitle-api', 'get', NODES, name, '-o', 'json')
    assert terminating['metadata']['deletionTimestamp']
    assert terminating['metadata']['finalizers'] == [f'{GROUP}/runtime-cleanup']
    kubectl('suntitle-proxy', 'patch', NODES, name, '--type=merge',
            '-p', '{"metadata":{"finalizers":[]}}', '-o', 'json')
    kubectl('suntitle-api', 'get', NODES, name, '-o', 'json', fails='NotFound')
    # Same name is a different resource incarnation.
    replacement = create('suntitle-api', 'SuntitleNode', name, node['spec'], {'ownerReferences': [owner]})
    assert replacement['metadata']['uid'] != node['metadata']['uid']
    print('PASS finalizer deletion and replacement UID')

    kubectl('suntitle-api', 'get', 'crds', '-o', 'json', fails='Forbidden')
    kubectl('suntitle-proxy', 'get', 'roles', '-o', 'json', fails='Forbidden')
    print('PASS runtime identities cannot administer bootstrap resources')


if __name__ == '__main__':
    try:
        main()
    finally:
        errors = []
        for resource, name in reversed(list(dict.fromkeys(created))):
            # A previously completed deletion is expected to be absent.
            existing = kubectl('admin', 'get', resource, name, '--ignore-not-found', '-o', 'json')
            if existing:
                try:
                    kubectl('admin', 'patch', resource, name, '--type=merge',
                            '-p', '{"metadata":{"finalizers":[]}}', '-o', 'json')
                    kubectl('admin', 'delete', resource, name, '--wait=false', '--ignore-not-found', '-o', 'name')
                except Exception as error:
                    errors.append(str(error))
        if errors:
            raise RuntimeError('Fixture cleanup failed: ' + '; '.join(errors))
