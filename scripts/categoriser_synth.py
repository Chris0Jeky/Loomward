"""Deterministic, invented evidence for LW-140; no inventory or model access."""
import argparse
import copy
import json
import random
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PACKET_SCHEMA = json.loads((ROOT / 'schemas/evidence-packet-v1.json').read_text(encoding='utf-8'))
RECORD_SCHEMA = json.loads((ROOT / 'schemas/categoriser-record-v1.json').read_text(encoding='utf-8'))
AXES = {s['properties']['axis']['const']: s['properties']['label']['enum']
        for s in RECORD_SCHEMA['$defs']['labels']['items']['oneOf']}


def check_schema_keywords(schema):
    """Refuse unsupported rules even in an unused property or schema alternative."""
    supported = {'$schema', 'title', 'description', '$defs', '$ref', 'oneOf', 'allOf', 'if',
                 'then', 'contains', 'minContains', 'maxContains', 'type', 'const', 'enum',
                 'properties', 'required', 'additionalProperties', 'maxItems', 'uniqueItems',
                 'items', 'minLength', 'maxLength', 'pattern', 'minimum', 'maximum'}
    unknown = schema.keys() - supported
    if unknown:
        raise ValueError('unsupported schema keyword: ' + ', '.join(sorted(unknown)))
    if 'additionalProperties' in schema and type(schema['additionalProperties']) is not bool:
        raise ValueError('unsupported schema keyword form: additionalProperties')
    for key in ('$defs', 'properties'):
        for child in schema.get(key, {}).values():
            check_schema_keywords(child)
    for key in ('oneOf', 'allOf'):
        for child in schema.get(key, []):
            check_schema_keywords(child)
    for key in ('if', 'then', 'contains', 'items'):
        if key in schema:
            check_schema_keywords(schema[key])
    if schema.get('$ref') == 'evidence-packet-v1.json':
        check_schema_keywords(PACKET_SCHEMA)


def check_structure(value, schema, document=RECORD_SCHEMA):
    """Validate the small schema vocabulary used by these two checked-in schemas, offline."""
    check_schema_keywords(schema)
    if document is not schema:
        check_schema_keywords(document)
    _check_structure(value, schema, document)


def _check_structure(value, schema, document):
    if '$ref' in schema:
        ref = schema['$ref']
        if ref == 'evidence-packet-v1.json':
            _check_structure(value, PACKET_SCHEMA, PACKET_SCHEMA)
        else:
            if not ref.startswith('#/'):
                raise ValueError('unsupported schema reference')
            target = document
            for key in ref[2:].split('/'):
                target = target[key]
            _check_structure(value, target, document)
    for candidate in schema.get('allOf', []):
        _check_structure(value, candidate, document)
    if 'if' in schema:
        try:
            _check_structure(value, schema['if'], document)
        except ValueError:
            pass
        else:
            _check_structure(value, schema.get('then', {}), document)
    if 'oneOf' in schema:
        matches = 0
        for candidate in schema['oneOf']:
            try:
                _check_structure(value, candidate, document)
                matches += 1
            except ValueError:
                pass
        if matches != 1:
            raise ValueError('value must match exactly one schema alternative')
    kinds = {'object': lambda v: isinstance(v, dict), 'array': lambda v: isinstance(v, list),
             'string': lambda v: isinstance(v, str), 'integer': lambda v: type(v) is int,
             'number': lambda v: type(v) in (int, float), 'boolean': lambda v: type(v) is bool,
             'null': lambda v: v is None}
    if 'type' in schema:
        types = schema['type'] if isinstance(schema['type'], list) else [schema['type']]
        if not any(kinds[k](value) for k in types):
            raise ValueError('incorrect value type')
    if 'const' in schema and (type(value) is not type(schema['const']) or value != schema['const']):
        raise ValueError('incorrect constant')
    if 'enum' in schema and value not in schema['enum']:
        raise ValueError('unknown enum value')
    if isinstance(value, dict):
        properties = schema.get('properties', {})
        if set(schema.get('required', [])) - value.keys():
            raise ValueError('missing required field')
        if schema.get('additionalProperties') is False and value.keys() - properties.keys():
            raise ValueError('unexpected field')
        for key, item in value.items():
            if key in properties:
                _check_structure(item, properties[key], document)
    elif isinstance(value, list):
        if len(value) > schema.get('maxItems', len(value)):
            raise ValueError('too many items')
        if schema.get('uniqueItems') and any(item in value[:i] for i, item in enumerate(value)):
            raise ValueError('duplicate array item')
        if 'contains' in schema:
            matches = 0
            for item in value:
                try:
                    _check_structure(item, schema['contains'], document)
                    matches += 1
                except ValueError:
                    pass
            if not schema.get('minContains', 1) <= matches <= schema.get('maxContains', len(value)):
                raise ValueError('incorrect contains count')
        for item in value:
            _check_structure(item, schema.get('items', {}), document)
    elif isinstance(value, str):
        if not schema.get('minLength', 0) <= len(value) <= schema.get('maxLength', len(value)):
            raise ValueError('incorrect string length')
        if 'pattern' in schema and re.search(schema['pattern'], value) is None:
            raise ValueError('incorrect string pattern')
    elif type(value) in (int, float):
        if not schema.get('minimum', value) <= value <= schema.get('maximum', value):
            raise ValueError('number outside schema bounds')


def validate_record(record):
    check_structure(record, RECORD_SCHEMA)
    for source in ('human_labels', 'teacher_labels'):
        axes = [item['axis'] for item in record[source]]
        if len(axes) != len(set(axes)):
            raise ValueError('duplicate label axis')


# meaning, extension family, extension mix, markers, recreatability
TEMPLATES = [
    ('source_project', 'code', ('rs', 'toml'), ('cargo_toml', 'git_dir'), 'unique'),
    ('generated_build', 'executable', ('rlib', 'pdb'), ('cargo_target_dir',), 'generated'),
    ('dependency_install', 'code', ('js', 'json'), ('node_modules_dir', 'pnpm_lock'), 'redownloadable'),
    ('package_cache', 'archive', ('tgz', 'zip'), ('cachedir_tag',), 'redownloadable'),
    ('model_weights', 'model', ('gguf', 'safetensors'), ('gguf_file', 'hf_hub_layout'), 'redownloadable'),
    ('virtual_disk', 'system', ('vhdx',), ('vhdx_file',), 'mixed'),
    ('photo_shoot', 'image', ('raw', 'xmp'), ('dcim_layout', 'raw_xmp_pairs'), 'unique'),
    ('photo_catalogue', 'data', ('lrcat', 'lrdata'), ('lightroom_catalog', 'lightroom_previews'), 'mixed'),
    ('catalogue_backup', 'archive', ('zip',), ('lightroom_backups_dated',), 'unique'),
    ('document_collection', 'document', ('pdf', 'docx'), ('office_documents',), 'unique'),
    ('archive', 'archive', ('zip', '7z'), ('archive_files',), 'unknown'),
    ('download', 'other', ('pdf', 'zip', 'bin'), (), 'unknown'),
    ('installer', 'executable', ('msi', 'exe'), ('installer_files',), 'redownloadable'),
    ('agent_worktree', 'code', ('py', 'toml'), ('git_file_worktree', 'pyproject_toml'), 'unique'),
]


def generate_records(count, seed):
    if count < 0:
        raise ValueError('count must be nonnegative')
    rng = random.Random(seed)
    records = []
    # Three views of the same instance always keep their lineage key together.
    for i in range(count):
        instance, variant = divmod(i, 3)
        meaning, family, extensions, markers, recreation = TEMPLATES[instance % len(TEMPLATES)]
        if variant == 0:
            files = rng.randint(40, 5000) * (2 if meaning == 'photo_shoot' else 1)
            if meaning in ('model_weights', 'virtual_disk', 'photo_catalogue'):
                files = rng.randint(2, 8)
            per_file = rng.randint(1000, 400_000)
            if meaning in ('model_weights', 'virtual_disk'):
                per_file *= 100_000
            elif meaning == 'photo_shoot':
                per_file *= 100
            total = files * per_file
            age_seconds = rng.randrange(1200 * 86400)
            lifecycle = ('active' if age_seconds < 30 * 86400 else
                         'dormant' if age_seconds < 730 * 86400 else 'historical')
            observed = 1_800_000_000 + rng.randint(0, 30) * 86400
            newest = observed - age_seconds
            group = f'synth-group-{instance:06d}'
            packet = {
                'schema': 'loomward.evidence-packet.v1', 'synthetic': True,
                'packet_id': f'synth-packet-{i:06d}', 'group_id': group, 'observed_at': observed,
                'subject': {'kind': 'folder', 'name_tokens': [rng.choice(('velnora', 'quorbin', 'zintrel')),
                                                                        'sample'], 'depth': rng.randint(1, 4)},
                'shape': {'files': files, 'folders': rng.randint(1, min(files, 30)), 'logical_bytes': total,
                          'allocated_bytes': total, 'max_depth': 6},
                'families': [{'family': family, 'share_files': 1.0, 'share_bytes': 1.0}],
                'extensions': [], 'markers': list(markers),
                'time': {'newest_content_write': newest, 'oldest_content_write': newest - 90 * 86400,
                         'newest_source_write': newest if recreation == 'unique' else None,
                         'newest_generated_write': newest if recreation == 'generated' else None,
                         'git_last_commit': newest if 'git_dir' in markers else None,
                         'git_dirty': age_seconds < 30 * 86400 if any(m.startswith('git_') for m in markers) else None,
                         'last_access': 'not_evidence'},
                'duplicates': {'duplicate_bytes': None, 'groups': None}, 'media': None,
                'coverage': {'state': 'complete', 'allocation_known': True,
                             'link_counts_observed': True, 'online': True},
            }
            remaining_files, remaining_bytes = files, total
            for index, ext in enumerate(extensions):
                n = remaining_files if index == len(extensions) - 1 else files // len(extensions)
                size = remaining_bytes if index == len(extensions) - 1 else total * n // files
                packet['extensions'].append({'ext': ext, 'files': n, 'bytes': size})
                remaining_files -= n
                remaining_bytes -= size
            if meaning == 'photo_shoot':
                raw = files // 2
                packet['extensions'][0]['bytes'] = total - raw * 1000
                packet['extensions'][1]['bytes'] = raw * 1000
                packet['media'] = {'raw_files': raw, 'sidecar_files': raw, 'raw_with_sidecar': raw,
                                   'capture_start': newest - 3600, 'capture_end': newest,
                                   'camera_models': 1}
            base_packet = copy.deepcopy(packet)
        else:
            packet = copy.deepcopy(base_packet)
            packet['packet_id'] = f'synth-packet-{i:06d}'
        if variant == 1:
            packet['markers'] = []
            packet['coverage'].update(state='partial', allocation_known=False,
                                      link_counts_observed=False)
            packet['shape'].update(logical_bytes=None, allocated_bytes=None)
            for ext in packet['extensions']:
                ext['bytes'] = None
            for family_entry in packet['families']:
                family_entry['share_bytes'] = None
            # Sparse evidence does not inherit a confident label from its complete sibling.
            labels = dict.fromkeys(AXES, 'unknown')
        elif variant == 2:
            packet['markers'] = ['cargo_target_dir', 'lightroom_catalog']
            packet['families'] = [{'family': 'code', 'share_files': .5, 'share_bytes': .5},
                                  {'family': 'document', 'share_files': .5, 'share_bytes': .5}]
            n, size = packet['shape']['files'], packet['shape']['logical_bytes']
            packet['extensions'] = [{'ext': 'txt', 'files': n // 2, 'bytes': size // 2},
                                    {'ext': 'bin', 'files': n - n // 2, 'bytes': size - size // 2}]
            packet['media'] = None
            labels = {'meaning': 'unknown', 'lifecycle': lifecycle, 'recreatability': 'unknown'}
        else:
            labels = {'meaning': meaning, 'lifecycle': lifecycle, 'recreatability': recreation}
        record = {'schema': 'loomward.categoriser-record.v1', 'synthetic': True,
                  'record_id': f'synth-record-{i:06d}', 'packet': packet,
                  'human_labels': [{'axis': axis, 'label': label} for axis, label in labels.items()],
                  'teacher_labels': [], 'summary_facts': ['/shape/files', '/shape/logical_bytes',
                                                        '/extensions', '/markers',
                                                        '/time/newest_content_write', '/coverage/state']}
        validate_record(record)
        records.append(record)
    return records


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--count', type=int, required=True)
    parser.add_argument('--seed', type=int, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args(argv)
    if args.count < 0:
        parser.error('count must be nonnegative')
    records = generate_records(args.count, args.seed)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(records, indent=2) + '\n', encoding='utf-8')
    print(f'Wrote {len(records)} synthetic records to {args.out}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
