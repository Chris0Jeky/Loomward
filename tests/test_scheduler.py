import concurrent.futures
import copy
import unittest
from loomward.scheduler import admit_workloads, LeaseBroker

DIMS = ('cpu_slots', 'memory_mib', 'gpu_mib', 'io_mib_s')
def vector(**kw): return {k: kw.get(k, 0) for k in DIMS}
def demand(): return vector(cpu_slots=1, memory_mib=100)
def scenario():
    return {'capacity': vector(cpu_slots=2, memory_mib=300, gpu_mib=0, io_mib_s=50),
            'reserved': vector(), 'telemetry_age_seconds': 0,
            'jobs': [{'id': 'build', 'demand': demand(), 'priority': 8, 'waited_seconds': 0},
                     {'id': 'index', 'demand': demand(), 'priority': 2, 'waited_seconds': 0},
                     {'id': 'embed', 'demand': demand(), 'priority': 1, 'waited_seconds': 0}]}

class AdmissionTests(unittest.TestCase):
    def test_does_not_overcommit_any_dimension(self):
        s = scenario(); r = admit_workloads(s)
        self.assertEqual([x['job_id'] for x in r['accepted']], ['build', 'index'])
        self.assertEqual(r['remaining']['cpu_slots'], 0)
        self.assertFalse(r['processes_changed'])

    def test_existing_reservations_count_once(self):
        s = scenario(); s['reserved'] = demand()
        self.assertEqual(len(admit_workloads(s)['accepted']), 1)

    def test_unknown_capacity_blocks_only_positive_demand(self):
        s = scenario(); s['capacity']['memory_mib'] = None
        self.assertEqual(admit_workloads(s)['accepted'], [])
        s = scenario(); s['capacity']['gpu_mib'] = None
        self.assertEqual(len(admit_workloads(s)['accepted']), 2)

    def test_stale_telemetry_blocks_admission(self):
        s = scenario(); s['telemetry_age_seconds'] = 31
        r = admit_workloads(s)
        self.assertEqual(r['accepted'], [])
        self.assertTrue(all('stale_capacity_observation' in x['reasons'] for x in r['deferred']))

    def test_age_priority_never_bypasses_budget(self):
        s = scenario(); s['jobs'][2].update(waited_seconds=3600, demand=vector(memory_mib=301))
        r = admit_workloads(s)
        self.assertNotIn('embed', [x['job_id'] for x in r['accepted']])
        self.assertEqual(len(r['accepted']), 2)

    def test_capacity_shrink_is_reported_not_faked(self):
        s = scenario(); s['reserved'] = vector(cpu_slots=3, memory_mib=500)
        r = admit_workloads(s)
        self.assertIn('cpu_slots', r['overcommitted'])
        self.assertEqual(r['accepted'], [])

    def test_invalid_numbers_fields_and_duplicates(self):
        for bad in (-1, True, 1.1):
            s = scenario(); s['capacity']['cpu_slots'] = bad
            with self.assertRaises(ValueError): admit_workloads(s)
        s = scenario(); s['jobs'].append(copy.deepcopy(s['jobs'][0]))
        with self.assertRaises(ValueError): admit_workloads(s)
        s = scenario(); s['capacity']['shell'] = 'run'
        with self.assertRaises(ValueError): admit_workloads(s)

    def test_each_resource_can_block(self):
        for dim in DIMS:
            s = scenario(); s['jobs'] = [dict(s['jobs'][0], demand=vector(**{dim: 1000}))]
            self.assertEqual(admit_workloads(s)['accepted'], [], dim)

    def test_no_input_mutation(self):
        s = scenario(); expected = copy.deepcopy(s); admit_workloads(s)
        self.assertEqual(s, expected)

class LeaseTests(unittest.TestCase):
    def setUp(self):
        self.now = 100.0
        self.b = LeaseBroker(vector(cpu_slots=1, memory_mib=100), clock=lambda: self.now)

    def acquire(self, rid='r1', owner='owner'):
        return self.b.acquire(owner, rid, demand(), ttl_seconds=30)

    def test_atomic_vector_admission_and_release(self):
        a = self.acquire(); b = self.acquire('r2')
        self.assertEqual(a['status'], 'granted'); self.assertEqual(b['status'], 'deferred')
        self.assertTrue(self.b.release(a['lease_id'], 'owner', a['token']))
        self.assertEqual(self.acquire('r3')['status'], 'granted')

    def test_replay_does_not_extend_expiry_or_duplicate_reservation(self):
        a = self.acquire(); self.now += 10; again = self.acquire()
        self.assertEqual(a['expires_at'], again['expires_at'])
        self.assertEqual(a['lease_id'], again['lease_id'])
        self.assertEqual(self.b.snapshot()['reserved']['cpu_slots'], 1)

    def test_changed_request_rejected(self):
        self.acquire()
        with self.assertRaises(ValueError): self.b.acquire('owner', 'r1', vector(cpu_slots=1), ttl_seconds=30)

    def test_expired_idempotency_key_does_not_resurrect(self):
        self.acquire(); self.now += 31
        self.assertEqual(self.acquire()['status'], 'expired')
        self.assertEqual(self.acquire('new')['status'], 'granted')

    def test_owner_and_token_checked(self):
        a = self.acquire()
        with self.assertRaises(PermissionError): self.b.release(a['lease_id'], 'other', a['token'])
        with self.assertRaises(PermissionError): self.b.release(a['lease_id'], 'owner', 'wrong')
        self.assertEqual(self.b.snapshot()['reserved']['cpu_slots'], 1)

    def test_snapshot_does_not_leak_tokens(self):
        a = self.acquire()
        self.assertNotIn(a['token'], str(self.b.snapshot()))

    def test_clock_rollback_fails_closed(self):
        self.acquire(); self.now = 99
        with self.assertRaises(ValueError): self.b.snapshot()

    def test_parallel_requests_get_only_one_reservation(self):
        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
            results = list(pool.map(lambda i: self.acquire(str(i)), range(20)))
        self.assertEqual(sum(r['status'] == 'granted' for r in results), 1)
        self.assertEqual(self.b.snapshot()['reserved']['memory_mib'], 100)

    def test_bad_ttl_and_boolean_are_rejected(self):
        for ttl in (0, True, 3601, 2.5):
            with self.assertRaises(ValueError): self.b.acquire('owner', 'bad', demand(), ttl_seconds=ttl)

    def test_deferred_replay_is_stable_requires_new_key(self):
        a = self.acquire(); self.acquire('denied')
        self.b.release(a['lease_id'], 'owner', a['token'])
        self.assertEqual(self.acquire('denied')['status'], 'deferred')
        self.assertEqual(self.acquire('new')['status'], 'granted')

class LeaseBoundaryRegressionTests(unittest.TestCase):
    def test_non_ascii_token_is_permission_denied_not_type_error(self):
        broker=LeaseBroker(vector(cpu_slots=1))
        lease=broker.acquire('owner','request',vector(cpu_slots=1))
        with self.assertRaises(PermissionError):
            broker.release(lease['lease_id'],'owner','é')
    def test_exposed_capacity_is_a_copy_not_mutable_authority(self):
        broker=LeaseBroker(vector(cpu_slots=1))
        broker.capacity['cpu_slots']=100
        result=broker.acquire('owner','request',vector(cpu_slots=2))
        self.assertEqual(result['status'],'deferred')
