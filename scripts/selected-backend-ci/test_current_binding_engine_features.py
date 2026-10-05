"""Pure control (no allocation, no build): the engine feature equality in
current_binding.py is exact and coherent with the record-after guard.

    python3 scripts/selected-backend-ci/test_current_binding_engine_features.py
"""
import importlib.util
import os
import re
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent


def load_binding():
    with tempfile.TemporaryDirectory() as d:
        os.environ.setdefault('FVOCI_CI_OWNER', 'control')
        os.environ.setdefault('FVOCI_CI_SELECTED_RUNS', d)
        spec = importlib.util.spec_from_file_location('current_binding', HERE / 'current_binding.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module


class EngineFeatures(unittest.TestCase):
    def test_exact_current_engine_feature_list(self):
        b = load_binding()
        self.assertTrue(b.engine_features_match(['default', 'worker']))
        self.assertTrue(b.engine_features_match(['worker', 'default']))
        self.assertFalse(b.engine_features_match(['worker']), 'missing declared default')
        self.assertFalse(b.engine_features_match(['default']), 'missing worker')
        self.assertFalse(b.engine_features_match(['default', 'test-hang', 'worker']), 'extra test-hang')
        self.assertFalse(b.engine_features_match(['default', 'worker', 'worker']))
        self.assertFalse(b.engine_features_match([]))

    def test_coherent_with_record_after_guard(self):
        b = load_binding()
        source = (HERE.parents[1] / 'scripts' / 'run-selected-backend-e2e.py').read_text()
        match = re.search(r"expected=\[([^\]]*)\] if name=='collab-engine'", source)
        self.assertIsNotNone(match, 'record-after guard must spell the engine expectation')
        guard = sorted(x.strip().strip("'\"") for x in match.group(1).split(','))
        self.assertEqual(guard, b.ENGINE_FEATURES)
        self.assertEqual(sorted(b.ENGINE_FEATURES), b.ENGINE_FEATURES)


if __name__ == '__main__':
    unittest.main()
