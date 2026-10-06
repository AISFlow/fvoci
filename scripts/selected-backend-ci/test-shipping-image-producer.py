#!/usr/bin/env python3
"""Pure source/artifact/resource controls; no Docker, compiler, DB or browser."""

import copy
import gzip
import importlib.util
import io
import json
import os
import struct
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
spec = importlib.util.spec_from_file_location("shipping_producer", HERE / "shipping-image-producer.py")
P = importlib.util.module_from_spec(spec); spec.loader.exec_module(P)


def archive(rows):
    out = io.BytesIO()
    with tarfile.open(fileobj=out, mode="w") as t:
        for row in rows:
            name, data, mode = row[:3]
            member = tarfile.TarInfo(name); member.size = len(data); member.mode = mode
            if len(row) > 3: member.uid = row[3]
            t.addfile(member, io.BytesIO(data))
    return out.getvalue()


def image_fixture(directory, mutation=None, oci=False, compressed=False):
    elf = bytearray(64); elf[:6] = b"\x7fELF\x02\x01"; struct.pack_into("<H", elf, 18, 62)
    rows = [("opt/fvoci/bin/" + x, bytes(elf) + (P.PRODUCT_SHA.encode() if x == "fvoci-server" else b""), 0o755) for x in sorted(P.BINARIES)]
    rows += [("opt/fvoci/static/index.html", b"<html>pure fixture</html>", 0o644),
             ("usr/lib/os-release", b'ID=ubuntu\nVERSION_ID="26.04"\n', 0o644)]
    config = {"os": "linux", "architecture": "amd64", "config": {"Env": sorted(P.EXPECTED_ENV), "User": "fvoci",
              "WorkingDir": "/opt/fvoci", "ExposedPorts": {"8080/tcp": {}}, "Labels": P.EXPECTED_LABELS.copy(), "Entrypoint": ["/opt/fvoci/bin/fvoci-migrate", "--start"]}}
    if mutation:
        mutation(rows, config)
    layer = archive(rows)
    config["rootfs"] = {"type": "layers", "diff_ids": ["sha256:" + P.digest(layer)]}
    if compressed: layer = gzip.compress(layer, mtime=0)
    conf = P.encoded(config); image_id = "sha256:" + P.digest(conf)
    manifest = [{"Config": "config.json", "RepoTags": ["fvoci:fixture"], "Layers": ["layer.tar"]}]
    members = [("config.json", conf, 0o600), ("manifest.json", P.encoded(manifest), 0o600), ("layer.tar", layer, 0o600)]
    if oci:
        leaf = P.encoded({"schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {"digest": image_id, "size": len(conf)}, "layers": [{"digest": "sha256:"+P.digest(layer), "size": len(layer)}]})
        image_id = "sha256:"+P.digest(leaf)
        members.append(("blobs/sha256/"+P.digest(leaf), leaf, 0o600))
    saved = archive(members)
    p = directory / "shipping-image.tar"; p.write_bytes(saved); p.chmod(0o600)
    return p, image_id


def layered_image_fixture(directory, following_rows, compressed=False, trailing=b""):
    path, _ = image_fixture(directory, oci=True)
    with tarfile.open(path) as t:
        config = json.load(t.extractfile("config.json"))
        layers = [t.extractfile("layer.tar").read(), archive(following_rows) + trailing]
    config["rootfs"]["diff_ids"] = ["sha256:" + P.digest(b) for b in layers]
    conf = P.encoded(config)
    saved_layers = [gzip.compress(b, mtime=0) if compressed else b for b in layers]
    names = ["layer1.tar", "layer2.tar"]
    manifest = [{"Config": "config.json", "RepoTags": ["fvoci:fixture"], "Layers": names}]
    leaf = P.encoded({"schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {"digest": "sha256:" + P.digest(conf), "size": len(conf)},
        "layers": [{"digest": "sha256:" + P.digest(b), "size": len(b)} for b in saved_layers]})
    image_id = "sha256:" + P.digest(leaf)
    path.write_bytes(archive([("config.json", conf, 0o600), ("manifest.json", P.encoded(manifest), 0o600),
        *[(n, b, 0o600) for n, b in zip(names, saved_layers, strict=True)],
        ("blobs/sha256/" + P.digest(leaf), leaf, 0o600)]))
    return path, image_id


class DecodedLayerControls(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def inspect_and_public(self, path, image_id):
        image = P.inspect_saved_image(path, image_id)
        receipt = {"image": image, "saved_image": {"sha256": P.digest(path.read_bytes()), "bytes": path.stat().st_size}}
        inputs = {"fixture": {"sha256": P.digest(b"fixture")}}
        for name, value in [("producer-receipt.json", receipt), ("tracked-inputs.json", inputs)]:
            file = self.root / name; file.write_bytes(P.encoded(value)); file.chmod(0o600)
        P.validate_public(self.root, receipt, inputs)
        return image

    def test_compressed_private_member_canary_refused(self):
        p, image_id = layered_image_fixture(self.root, [("private/credential-canary", P.CANARY, 0o600)], compressed=True)
        self.assertNotIn(P.CANARY, p.read_bytes())
        with self.assertRaisesRegex(P.Refusal, "ARTIFACT_CANARY"): self.inspect_and_public(p, image_id)

    def test_decoded_canary_crosses_stream_chunk(self):
        payload = b"x" * (1024**2 - 512 - len(P.CANARY)//2) + P.CANARY
        rows = [("private/credential-canary", payload, 0o600)]
        decoded = archive(rows); at = decoded.index(P.CANARY)
        self.assertLess(at, 1024**2); self.assertGreater(at + len(P.CANARY), 1024**2)
        p, image_id = layered_image_fixture(self.root, rows, compressed=True)
        self.assertNotIn(P.CANARY, p.read_bytes())
        with self.assertRaisesRegex(P.Refusal, "ARTIFACT_CANARY"): self.inspect_and_public(p, image_id)

    def test_decoded_canary_after_tar_end_refused(self):
        # The tar reader stops before these trailing bytes; the full decoded
        # stream identity/privacy scan must still examine them.
        p, image_id = layered_image_fixture(self.root, [], compressed=True, trailing=P.CANARY)
        self.assertNotIn(P.CANARY, p.read_bytes())
        with self.assertRaisesRegex(P.Refusal, "ARTIFACT_CANARY"): self.inspect_and_public(p, image_id)

    def test_decoded_scan_keeps_carry_across_short_reads(self):
        class ShortReads(io.BytesIO):
            def read(self, size=-1):
                return super().read(min(size, 3))
        with self.assertRaisesRegex(P.Refusal, "ARTIFACT_CANARY"):
            P.hash_stream(ShortReads(b"prefix" + P.CANARY + b"suffix"), scan_canary=True)
        self.assertEqual(P.hash_stream(ShortReads(b"valid decoded bytes"), scan_canary=True)[0], P.digest(b"valid decoded bytes"))

    def test_protected_ancestor_output_and_os_whiteouts_refused(self):
        whiteouts = [".wh.opt", ".wh.etc", ".wh.usr", ".wh..wh..opq",
            "opt/.wh.fvoci", "opt/.wh..wh..opq", "opt/fvoci/.wh.bin", "opt/fvoci/.wh.static", "opt/fvoci/.wh..wh..opq",
            "opt/fvoci/bin/.wh..wh..opq", *["opt/fvoci/bin/.wh." + b for b in sorted(P.BINARIES)],
            "opt/fvoci/static/.wh..wh..opq", "opt/fvoci/static/.wh.index.html", "opt/fvoci/static/assets/.wh..wh..opq",
            "etc/.wh.os-release", "etc/.wh..wh..opq", "usr/.wh.lib", "usr/.wh..wh..opq",
            "usr/lib/.wh.os-release", "usr/lib/.wh..wh..opq"]
        for compressed in [False, True]:
            for name in whiteouts:
                with self.subTest(name=name, compressed=compressed):
                    p, image_id = layered_image_fixture(self.root, [(name, b"", 0o600)], compressed=compressed)
                    with self.assertRaises(P.Refusal): self.inspect_and_public(p, image_id)

    def test_valid_multiple_layers_compressed_and_plain(self):
        for compressed in [False, True]:
            with self.subTest(compressed=compressed):
                p, image_id = layered_image_fixture(self.root, [
                    ("var/.wh.cache", b"", 0o600), ("var/lib/cache/.wh..wh..opq", b"", 0o600),
                    ("opt/fvoci/static/assets/added.js", b"export const valid = true;", 0o644),
                    ("usr/lib/os-release", b'ID=ubuntu\nVERSION_ID="26.04"\nNAME="Ubuntu"\n', 0o644)], compressed=compressed)
                image = self.inspect_and_public(p, image_id)
                self.assertEqual(set(image["binaries"]), P.BINARIES)
                self.assertIn("index.html", image["static_files"])
                self.assertIn("assets/added.js", image["static_files"])
                self.assertEqual(image["os_release_sha256"], P.digest(b'ID=ubuntu\nVERSION_ID="26.04"\nNAME="Ubuntu"\n'))


class SourceControls(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name) / "source"; self.root.mkdir()
        self.run_git("init", "-q")
        (self.root / ".gitignore").write_text("ignored-output\n")
        (self.root / "file").write_bytes(b"original")
        self.run_git("add", "."); self.run_git("-c", "user.name=Pure Fixture", "-c", "user.email=pure@example.invalid", "commit", "-qm", "fixture")
        self.sha = self.run_git("rev-parse", "HEAD").strip(); self.tree = self.run_git("rev-parse", "HEAD^{tree}").strip()

    def run_git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.root), *args], text=True, stderr=subprocess.DEVNULL)

    def verify(self):
        return P.checkout(self.root, self.sha, self.tree)

    def test_exact_tracked_snapshot(self):
        rows = self.verify(); target = Path(self.tmp.name) / "context"
        P.snapshot(self.root, target, rows)
        self.assertEqual((target / "file").read_bytes(), b"original")
        self.assertFalse((target / ".git").exists())
        self.assertEqual(rows["file"]["sha256"], P.digest(b"original"))
        P.verify_snapshot(target, rows)

    def test_physical_context_extra_bytes_and_xor0111(self):
        rows = self.verify(); target = Path(self.tmp.name) / "context"; P.snapshot(self.root, target, rows)
        extra = target / "secret"; extra.write_bytes(b"must not send")
        with self.assertRaisesRegex(P.Refusal, "CONTEXT_PATH_DRIFT"): P.verify_snapshot(target, rows)
        extra.unlink(); file = target / "file"; file.chmod(file.stat().st_mode ^ 0o111)
        with self.assertRaises(P.Refusal): P.verify_snapshot(target, rows)
        file.chmod(0o644); file.write_bytes(b"modified")
        with self.assertRaises(P.Refusal): P.verify_snapshot(target, rows)

    def test_wrong_head_and_tree(self):
        for sha, tree in [("a" * 40, self.tree), (self.sha, "a" * 40), ("HEAD", self.tree)]:
            with self.subTest(sha=sha, tree=tree), self.assertRaises(P.Refusal):
                P.checkout(self.root, sha, tree)

    def test_tracked_byte_drift(self):
        (self.root / "file").write_bytes(b"changed")
        with self.assertRaises(P.Refusal): self.verify()

    def test_exact_xor0111_mode_drift(self):
        file = self.root / "file"; file.chmod(file.stat().st_mode ^ 0o111)
        with self.assertRaises(P.Refusal): self.verify()

    def test_untracked_and_ignored_refusal(self):
        for name in ["untracked", "ignored-output"]:
            with self.subTest(name=name):
                p = self.root / name; p.write_bytes(b"must not be copied")
                with self.assertRaises(P.Refusal): self.verify()
                p.unlink()

    def test_symlink_and_submodule_type_refusal(self):
        file = self.root / "file"; file.unlink(); file.symlink_to(".gitignore")
        self.run_git("add", "file"); self.run_git("-c", "user.name=Pure Fixture", "-c", "user.email=pure@example.invalid", "commit", "-qm", "link")
        sha = self.run_git("rev-parse", "HEAD").strip(); tree = self.run_git("rev-parse", "HEAD^{tree}").strip()
        with self.assertRaises(P.Refusal): P.checkout(self.root, sha, tree)
        # Git submodule modes and arbitrary entry kinds share the closed type
        # boundary; use immutable tree data only, never initialize a submodule.
        original = P.git
        def rows(root, *args):
            if args[0] == "ls-tree": return b"160000 commit " + b"a" * 40 + b"\tmodule\0"
            return original(root, *args)
        with patch.object(P, "git", rows), self.assertRaises(P.Refusal): P.checkout(self.root, sha, tree)

    def test_snapshot_race_and_external_parent_symlink(self):
        rows = self.verify(); (self.root / "file").write_bytes(b"later")
        with self.assertRaises(P.Refusal): P.snapshot(self.root, Path(self.tmp.name) / "race", rows)
        for name in ["../escape", "/escape", "a//b", "a/./b", "a\\b", "x\n"]:
            self.assertFalse(P.safe_path(name))


class ResourceAndOwnershipControls(unittest.TestCase):
    def test_floors_exact_and_one_byte_below(self):
        P.admit({"free_bytes": P.DISK_FLOOR, "effective_mem_available": P.MEMORY_FLOOR})
        for measured in [{"free_bytes": P.DISK_FLOOR-1, "effective_mem_available": P.MEMORY_FLOOR},
                         {"free_bytes": P.DISK_FLOOR, "effective_mem_available": P.MEMORY_FLOOR-1}]:
            with self.assertRaisesRegex(P.Refusal, "RESOURCE_NOTADMITTED"): P.admit(measured)

    def test_cgroup_finite_max_and_current(self):
        with tempfile.TemporaryDirectory() as t:
            root = Path(t); (root / "memory.current").write_text("200")
            (root / "memory.max").write_text("1000"); self.assertEqual(P.cgroup_memory(root), 800)
            (root / "memory.max").write_text("max"); self.assertIsNone(P.cgroup_memory(root))

    def test_unadmitted_stops_before_snapshot_or_builder(self):
        with tempfile.TemporaryDirectory() as t:
            work = Path(t); producer = P.Producer(work, "fixture"); calls = []
            def command(stage, argv, monitored=False):
                calls.append(stage); return str(work).encode()
            with patch.object(P.platform, "system", return_value="Linux"), patch.object(P.platform, "machine", return_value="x86_64"), \
                    patch.object(Path, "read_text", return_value="ID=ubuntu\nVERSION_ID=26.04\n"), \
                    patch.object(P, "git", return_value=b"a"*40), patch.object(P, "checkout", return_value={str(n): {} for n in range(2134)}), \
                    patch.object(P, "resources", return_value={"free_bytes": P.DISK_FLOOR-1, "effective_mem_available": P.MEMORY_FLOOR}), \
                    patch.object(P, "__file__", str(work / "scripts/selected-backend-ci/shipping-image-producer.py")), \
                    patch.object(producer, "command", side_effect=command), patch.object(P, "snapshot") as snapshot:
                with self.assertRaisesRegex(P.Refusal, "RESOURCE_NOTADMITTED"): producer.build(work, work, "a"*40)
                snapshot.assert_not_called(); self.assertEqual(calls, ["docker-root"])

    def test_foreign_identity_or_caps_never_stopped(self):
        cid = "a"*64
        good = {"Id": cid, "Name": "/buildx_buildkit_fixture0", "Image": P.BUILDKIT_IMAGE.split("@", 1)[1],
                "HostConfig": {"Memory": P.BUILDER_MEMORY, "CpuPeriod": 100000, "CpuQuota": 200000, "PortBindings": {}},
                "State": {"Running": True, "Pid": 123}, "NetworkSettings": {"Ports": {}}}
        for path, value in [("Id", "b"*64), ("Name", "/foreign"), ("Image", "sha256:"+"b"*64), ("memory", P.BUILDER_MEMORY+1), ("cpu", 400000)]:
            with self.subTest(path=path), tempfile.TemporaryDirectory() as t:
                bad = copy.deepcopy(good)
                if path == "memory": bad["HostConfig"]["Memory"] = value
                elif path == "cpu": bad["HostConfig"]["CpuQuota"] = value
                else: bad[path] = value
                producer = P.Producer(Path(t), "fixture"); producer.cid = cid; calls = []
                def command(stage, argv, monitored=False): calls.append(argv); return P.encoded([bad])
                with patch.object(producer, "command", side_effect=command), self.assertRaises(P.Refusal): producer.stop_builder()
                self.assertFalse(any("stop" in a for a in calls))


class ArtifactControls(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name); image, image_id = image_fixture(self.root)
        outputs = P.inspect_saved_image(image, image_id)
        self.receipt = {"image": outputs, "saved_image": {"sha256": P.digest(image.read_bytes()), "bytes": image.stat().st_size}}
        self.inputs = {"fixture": {"sha256": P.digest(b"fixture")}}
        for name, value in [("producer-receipt.json", self.receipt), ("tracked-inputs.json", self.inputs)]:
            f = self.root / name; f.write_bytes(P.encoded(value)); f.chmod(0o600)

    def validate(self): P.validate_public(self.root, self.receipt, self.inputs)

    def test_exact_three_artifacts(self): self.validate()

    def test_docker29_manifest_and_config_distinct_identities(self):
        p, image_id = image_fixture(self.root, oci=True)
        row = P.inspect_saved_image(p, image_id)
        self.assertEqual(row["manifest_id"], image_id)
        self.assertNotEqual(row["config_id"], image_id)
        with self.assertRaises(P.Refusal): P.inspect_saved_image(p, "sha256:"+"f"*64)
        with tarfile.open(p) as t:
            members = [(m.name, t.extractfile(m).read(), m.mode) for m in t if m.isfile()]
        members = [(n, b+b"tamper" if n == "layer.tar" else b, m) for n, b, m in members]
        p.write_bytes(archive(members))
        with self.assertRaisesRegex(P.Refusal, "IMAGE_LAYER_DESCRIPTOR_MISMATCH"): P.inspect_saved_image(p, image_id)

    def test_compressed_oci_layers_keep_uncompressed_diff_identity(self):
        p, image_id = image_fixture(self.root, oci=True, compressed=True)
        row = P.inspect_saved_image(p, image_id)
        self.assertEqual(row["manifest_id"], image_id)
        self.assertEqual(set(row["binaries"]), P.BINARIES)

    def test_unknown_raw_trace_and_missing_file(self):
        p = self.root / "browser.log"; p.write_bytes(b"private")
        with self.assertRaises(P.Refusal): self.validate()
        p.unlink(); (self.root / "tracked-inputs.json").unlink()
        with self.assertRaises(P.Refusal): self.validate()

    def test_symlink_hardlink_mode_and_receipt_tamper(self):
        file = self.root / "producer-receipt.json"; original = file.read_bytes()
        file.write_bytes(original + P.CANARY)
        with self.assertRaises(P.Refusal): self.validate()
        file.write_bytes(original); file.chmod(0o644)
        with self.assertRaises(P.Refusal): self.validate()
        file.chmod(0o600)
        outside = self.root.parent / (self.root.name + "-link")
        os.link(file, outside)
        try:
            with self.assertRaises(P.Refusal): self.validate()
        finally: outside.unlink()
        file.unlink(); file.symlink_to(self.root / "tracked-inputs.json")
        with self.assertRaises(P.Refusal): self.validate()

    def test_binary_canary_fail_closed(self):
        p, image_id = image_fixture(self.root, lambda rows, cfg: rows.append(("canary", P.CANARY, 0o600)))
        self.receipt["image"] = P.inspect_saved_image(p, image_id)
        self.receipt["saved_image"] = {"sha256": P.digest(p.read_bytes()), "bytes": p.stat().st_size}
        (self.root / "producer-receipt.json").write_bytes(P.encoded(self.receipt))
        with self.assertRaisesRegex(P.Refusal, "ARTIFACT_CANARY"): self.validate()

    def test_config_credentials_and_output_path_arch_ownership(self):
        mutations = [lambda rows, c: c["config"]["Env"].append("DATABASE_URL=must-not-publish"),
                     lambda rows, c: c["config"].update(Labels={"private": "must-not-publish"}),
                     lambda rows, c: c["config"].update(Hostname="must-not-publish"),
                     lambda rows, c: c.update(architecture="arm64"),
                     lambda rows, c: rows.append(("opt/fvoci/static/../escape", b"bad", 0o644)),
                     lambda rows, c: rows.append(("opt/fvoci/static/.wh.index.html", b"", 0o644)),
                     lambda rows, c: rows.append(("opt/.wh.fvoci", b"", 0o644)),
                     lambda rows, c: rows.__setitem__(0, (rows[0][0], rows[0][1], 0o777)),
                     lambda rows, c: rows.__setitem__(0, (*rows[0], 1000)),
                     lambda rows, c: rows.__setitem__(2, (rows[2][0], bytes(64), 0o755))]
        for mutation in mutations:
            with self.subTest(mutation=mutation):
                p, image_id = image_fixture(self.root, mutation)
                with self.assertRaises(P.Refusal): P.inspect_saved_image(p, image_id)

    def test_layer_hash_and_server_stamp(self):
        def remove_stamp(rows, config):
            for index, row in enumerate(rows):
                if row[0].endswith("/fvoci-server"):
                    rows[index] = (row[0], row[1][:64], row[2])
        p, image_id = image_fixture(self.root, remove_stamp)
        with self.assertRaisesRegex(P.Refusal, "IMAGE_SOURCE_STAMP_MISSING"): P.inspect_saved_image(p, image_id)
        p, image_id = image_fixture(self.root)
        with tarfile.open(p) as t:
            manifest = t.extractfile("manifest.json").read(); conf = t.extractfile("config.json").read(); layer = t.extractfile("layer.tar").read()
        p.write_bytes(archive([("manifest.json", manifest, 0o600), ("config.json", conf, 0o600), ("layer.tar", layer+b"changed", 0o600)]))
        with self.assertRaisesRegex(P.Refusal, "IMAGE_LAYER_ID_MISMATCH"): P.inspect_saved_image(p, image_id)


class WorkflowControls(unittest.TestCase):
    def setUp(self):
        import yaml
        self.yaml = yaml
        self.data = yaml.safe_load((ROOT / ".github/workflows/install.yml").read_text())
        self.base = yaml.safe_load(subprocess.check_output(["git", "-C", str(ROOT), "show", P.PRODUCT_SHA + ":.github/workflows/install.yml"], text=True))

    def test_ordinary_jobs_permissions_and_timeouts_identical(self):
        for name in ["install-smoke", "backup-restore-smoke", "upgrade-smoke-arm64"]:
            self.assertEqual(self.data["jobs"][name], self.base["jobs"][name])
        for name in ["permissions", "concurrency"]: self.assertEqual(self.data[name], self.base[name])
        for name in ["pull_request", "push", "merge_group"]: self.assertEqual(self.data[True][name], self.base[True][name])

    def test_new_job_exact_pins_allowlist_no_runtime(self):
        job = self.data["jobs"]["shipping-image-producer"]
        self.assertEqual(set(job), {"needs", "if", "runs-on", "timeout-minutes", "steps"})
        self.assertEqual((job["runs-on"], job["timeout-minutes"]), ("ubuntu-26.04", 45))
        a, b, run, upload = job["steps"]
        self.assertEqual(a["with"], {"ref": "${{ github.sha }}", "path": "tooling", "persist-credentials": False})
        self.assertEqual(b["with"], {"ref": P.PRODUCT_SHA, "path": "product", "persist-credentials": False})
        self.assertEqual(set(upload["with"]["path"].splitlines()), {"${{ runner.temp }}/shipping-producer/public/" + p for p in P.PUBLIC_FILES})
        self.assertNotIn("if", upload); self.assertNotIn("secrets", json.dumps(job))
        self.assertEqual(run["env"], {"TOOLING_SHA": "${{ github.sha }}"})

    def test_missing_registry_entry_refuses_job(self):
        sys.path.insert(0, str(ROOT / "scripts")); import ci_selection as sel
        with patch.dict(sel.WORKFLOW_JOBS, {"install": tuple(j for j in sel.WORKFLOW_JOBS["install"] if j != "shipping-image-producer")}), \
                patch.dict(sel.OPT_IN_JOBS, {"install": {j: v for j, v in sel.OPT_IN_JOBS["install"].items() if j != "shipping-image-producer"}}):
            errors = sel.verify_workflow_registry(ROOT)
        self.assertIn("install: unregistered job id shipping-image-producer", errors)
        self.assertTrue(any("workflow_dispatch inputs must be exactly" in e for e in errors))

    def test_proposed_data_only_registry_overlay_closed(self):
        import ci_selection as sel
        with patch.dict(sel.WORKFLOW_JOBS, {"install": (*tuple(j for j in sel.WORKFLOW_JOBS["install"] if j != "shipping-image-producer"), "shipping-image-producer")}), \
                patch.dict(sel.OPT_IN_JOBS, {"install": {**sel.OPT_IN_JOBS["install"], "shipping-image-producer": "run_shipping_producer"}}), \
                patch.dict(sel.OPT_IN_RUNNER, {"shipping-image-producer": "ubuntu-26.04"}):
            self.assertEqual(sel.verify_workflow_registry(ROOT), [])
            for event in ["pull_request", "push", "merge_group"]:
                chosen, err = sel.dispatch_opt_ins("install", event, {"inputs": {"run_shipping_producer": "true"}})
                self.assertEqual((chosen, err), (frozenset(), None))
            for value, expected in [(True, True), ("true", True), (False, False), ("false", False)]:
                chosen, err = sel.dispatch_opt_ins("install", "workflow_dispatch", {"inputs": {"run_shipping_producer": value}})
                self.assertIsNone(err); self.assertEqual("run_shipping_producer" in chosen, expected)
            for value in ["TRUE", 1, None, {}, "maybe"]:
                chosen, err = sel.dispatch_opt_ins("install", "workflow_dispatch", {"inputs": {"run_shipping_producer": value}})
                self.assertEqual(err, "DISPATCH_INPUT_VALUE_INVALID")


class HostedExperimentalResourceControls(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.producer = P.Producer(self.root, "fixture", P.HOSTED_PROFILE)
        self.cid = "a" * 64
        self.live = {"Id": self.cid, "Name": "/buildx_buildkit_fixture0", "Image": P.BUILDKIT_IMAGE.split("@", 1)[1],
            "HostConfig": {"Memory": P.BUILDER_MEMORY, "CpuPeriod": 100000, "CpuQuota": 200000, "PortBindings": {}},
            "State": {"Running": True, "Pid": 123, "OOMKilled": False}, "NetworkSettings": {"Ports": {}}}

    def row(self, name, maximum=None, current=100):
        return {"path": name, "identity": name, "memory_max": maximum, "current": current, "peak": current,
                "cpu_max": ["200000", "100000"] if maximum == P.BUILDER_MEMORY else ["max", "100000"],
                "container_id": self.cid if maximum == P.BUILDER_MEMORY else None,
                "events": {k: 0 for k in ["low", "high", "max", "oom", "oom_kill", "oom_group_kill"]}}

    def measured(self, available):
        return {"free_bytes": P.DISK_FLOOR, "effective_mem_available": available,
                "host_mem_available": available, "cgroup_memory_available": None}

    def make_chain(self):
        proc = self.root / "proc"; mount = self.root / "cg"
        number = str(P.os.getpid()); (proc / number / "ns").mkdir(parents=True)
        (proc / "self").symlink_to(number); (proc / "1/ns").mkdir(parents=True)
        for pid in ["self", "1"]: (proc / pid / "ns/cgroup").symlink_to("cgroup:[1]")
        (mount / "parent/child").mkdir(parents=True)
        dev = mount.stat().st_dev; device = f"{P.os.major(dev)}:{P.os.minor(dev)}"
        (proc / "self/mountinfo").write_text(f"1 0 {device} / {mount} rw - cgroup2 cgroup rw\n2 0 {device} / {proc} rw - proc proc rw\n")
        (proc / "self/stat").write_text(number + " (fixture ) comm) S " + "0 "*18 + "100 0\n")
        (proc / "self/cgroup").write_text("0::/parent/child\n")
        (mount / "cgroup.controllers").write_text("cpu memory io\n")
        (mount / "cgroup.procs").write_text(number + "\n")
        for path in [mount / "parent", mount / "parent/child"]:
            for name, value in {"cgroup.type": "domain", "cgroup.procs": number + "\n" + number + "\n",
                                "memory.max": "max", "memory.current": "100", "memory.peak": "200",
                                "memory.events": "low 0\nhigh 0\nmax 0\noom 0\noom_kill 0\noom_group_kill 0\n",
                                "cpu.max": "max 100000"}.items(): (path / name).write_text(value)
        # Synthetic filesystem seam only: no installed stat or host reader runs.
        def filesystem(fd, expected):
            self.assertEqual(expected, b"63677270" if P.os.fstat(fd).st_ino == mount.stat().st_ino else b"9fa0")
            return "synthetic-tool-hash"
        mock = patch.object(P, "cgroup_filesystem", side_effect=filesystem); mock.start(); self.addCleanup(mock.stop)
        return proc, mount

    def test_start_running_and_disk_exact_plus_minus_one(self):
        for running, floor in [(False, P.BUILDER_MEMORY + P.HOST_RESERVE), (True, P.HOST_RESERVE)]:
            for delta in [-1, 0, 1]:
                with self.subTest(running=running, delta=delta):
                    measured = self.measured(floor + delta)
                    if delta < 0:
                        with self.assertRaisesRegex(P.Refusal, "RESOURCE_NOTADMITTED"): P.admit(measured, P.HOSTED_PROFILE, running=running)
                    else: P.admit(measured, P.HOSTED_PROFILE, running=running)
            for delta in [-1, 0, 1]:
                measured = self.measured(floor); measured["free_bytes"] += delta
                if delta < 0:
                    with self.assertRaisesRegex(P.Refusal, "RESOURCE_NOTADMITTED"): P.admit(measured, P.HOSTED_PROFILE, running=running)
                else: P.admit(measured, P.HOSTED_PROFILE, running=running)
        self.assertEqual(P.Producer(self.root, "legacy").profile, P.LEGACY_PROFILE)
        with self.assertRaisesRegex(P.Refusal, "RESOURCE_NOTADMITTED"): P.admit(self.measured(15330836480))
        P.admit(self.measured(15330836480), P.HOSTED_PROFILE)
        with self.assertRaisesRegex(P.Refusal, "RESOURCE_PROFILE_INVALID"): P.admit(self.measured(P.MEMORY_FLOOR), "unknown")

    def test_full_finite_ancestor_chain_and_host_root(self):
        proc, mount = self.make_chain()
        (mount / "parent/memory.max").write_text(str(P.BUILDER_MEMORY + P.HOST_RESERVE + 100))
        rows = P.cgroup_chain(root=mount, proc=proc)
        self.assertEqual(len(rows), 2)
        self.assertEqual(rows[0]["memory_max"] - rows[0]["current"], P.BUILDER_MEMORY + P.HOST_RESERVE)
        self.assertIsNone(rows[1]["memory_max"])
        with patch.object(P, "cgroup_chain", return_value=rows), patch.object(P, "resources", return_value=self.measured(P.MEMORY_FLOOR)):
            self.producer.check_resources("preflight")
            rows[0]["current"] += 1
            with self.assertRaisesRegex(P.Refusal, "RESOURCE_NOTADMITTED"): self.producer.check_resources("preflight")
        (proc / "self/cgroup").write_text("0::/\n")
        self.assertEqual(P.cgroup_chain(root=mount, proc=proc), [])

    def test_hidden_chain_missing_unknown_negative_or_duplicate_metadata(self):
        proc, mount = self.make_chain()
        mutations = [(proc / "self/mountinfo", f"1 0 0:1 /hidden {mount} rw - cgroup2 cgroup rw\n"),
            (proc / "self/cgroup", "0::/../hidden\n"), (proc / "self/cgroup", "1:memory:/legacy\n"),
            (mount / "cgroup.controllers", "cpu\n"), (mount / "parent/memory.max", "unknown"),
            (mount / "parent/memory.current", "-1"), (mount / "parent/memory.peak", "99"),
            (mount / "parent/cpu.max", "unknown 100000"), (mount / "parent/memory.events", "oom 0\n"),
            (mount / "parent/memory.events", "oom 0\noom 1\noom_kill 0\noom_group_kill 0\n")]
        for file, value in mutations:
            with self.subTest(file=file.name, value=value):
                original = file.read_text(); file.write_text(value)
                try:
                    with self.assertRaises(P.Refusal): P.cgroup_chain(root=mount, proc=proc)
                finally: file.write_text(original)
        file = mount / "parent/memory.peak"; data = file.read_bytes(); file.unlink()
        with self.assertRaisesRegex(P.Refusal, "CGROUP_METADATA_UNKNOWN"): P.cgroup_chain(root=mount, proc=proc)
        file.write_bytes(data)
        (proc / "1/ns/cgroup").unlink(); (proc / "1/ns/cgroup").symlink_to("cgroup:[2]")
        # ROOT-approved old namespace-equality negative maps to a hidden subtree.
        (mount / "cgroup.type").write_text("domain")
        with self.assertRaisesRegex(P.Refusal, "CGROUP_HOST_ROOT_INVALID"): P.cgroup_chain(root=mount, proc=proc)

    def test_verified_builder_counters_ancestors_and_no_available_credit(self):
        self.producer.cid = self.cid; self.producer.builder_created = True
        parent = self.row("ancestor", P.MEMORY_FLOOR, P.MEMORY_FLOOR-P.HOST_RESERVE)
        leaf = self.row("builder", P.BUILDER_MEMORY, P.BUILDER_MEMORY-1)
        def chain(pid="self", **kw): return [self.row("launcher")] if pid == "self" else [parent, leaf]
        with patch.object(P, "cgroup_chain", side_effect=chain), patch.object(self.producer, "owned_builder", return_value=self.live), \
                patch.object(P, "resources", return_value=self.measured(P.HOST_RESERVE)):
            measured = self.producer.check_resources("shipping-build", running=True)
            self.assertEqual(measured["effective_mem_available"], P.HOST_RESERVE)
            self.assertEqual(self.producer.profile_receipt()["cgroups"]["builder"]["peak"], P.BUILDER_MEMORY-1)
            parent["current"] += 1
            with self.assertRaisesRegex(P.Refusal, "RESOURCE_NOTADMITTED"): self.producer.check_resources("shipping-build", running=True)
        producer = P.Producer(self.root, "fixture", P.HOSTED_PROFILE); producer.cid = self.cid; producer.builder_created = True
        parent["memory_max"] = 8*1024**3; parent["current"] = 100; parent["peak"] = 100
        with patch.object(P, "cgroup_chain", side_effect=chain), patch.object(producer, "owned_builder", return_value=self.live), \
                patch.object(P, "resources", return_value=self.measured(P.MEMORY_FLOOR)):
            with self.assertRaisesRegex(P.Refusal, "BUILDER_ANCESTOR_BUDGET_INVALID"): producer.check_resources("shipping-build", running=True)

    def test_builder_unknown_cap_cpu_pid_identity_or_counter_refused(self):
        mutations = [lambda r: r.update(memory_max=None), lambda r: r.update(memory_max=P.BUILDER_MEMORY+1),
            lambda r: r.update(cpu_max=["400000", "100000"]), lambda r: r.update(current=P.BUILDER_MEMORY+1, peak=P.BUILDER_MEMORY+1),
            lambda r: r.update(container_id="b"*64), lambda r: r["events"].update(oom=1),
            lambda r: r["events"].update(oom_kill=1), lambda r: r["events"].update(oom_group_kill=1)]
        for change in mutations:
            self.producer = P.Producer(self.root, "fixture", P.HOSTED_PROFILE)
            self.producer.cid = self.cid; self.producer.builder_created = True
            leaf = self.row("builder", P.BUILDER_MEMORY); change(leaf)
            with patch.object(P, "cgroup_chain", side_effect=lambda pid="self", **kw: [] if pid == "self" else [leaf]), \
                    patch.object(self.producer, "owned_builder", return_value=self.live), patch.object(P, "resources", return_value=self.measured(P.MEMORY_FLOOR)):
                with self.assertRaises(P.Refusal): self.producer.check_resources("shipping-build", running=True)
        for field, value in [("Pid", 0), ("OOMKilled", True), ("Running", False)]:
            live = copy.deepcopy(self.live); live["State"][field] = value
            with patch.object(P, "cgroup_chain", return_value=[]), patch.object(self.producer, "owned_builder", return_value=live), \
                    patch.object(P, "resources", return_value=self.measured(P.MEMORY_FLOOR)):
                with self.assertRaises(P.Refusal): self.producer.check_resources("shipping-build", running=True)
        self.producer.cid = None
        with patch.object(P, "cgroup_chain", return_value=[]), patch.object(self.producer, "command", return_value=b""), \
                patch.object(P, "resources", return_value=self.measured(P.MEMORY_FLOOR)):
            with self.assertRaisesRegex(P.Refusal, "BUILDER_METADATA_UNKNOWN"): self.producer.check_resources("shipping-build", running=True)

    def test_no_builder_usage_added_to_available_memory(self):
        self.producer.cid = self.cid; self.producer.builder_created = True
        leaf = self.row("builder", P.BUILDER_MEMORY, P.BUILDER_MEMORY-1)
        with patch.object(P, "cgroup_chain", side_effect=lambda pid="self", **kw: [] if pid == "self" else [leaf]), \
                patch.object(self.producer, "owned_builder", return_value=self.live), patch.object(P, "resources", return_value=self.measured(P.HOST_RESERVE-1)):
            with self.assertRaisesRegex(P.Refusal, "RESOURCE_NOTADMITTED"): self.producer.check_resources("shipping-build", running=True)

    def test_oom_increment_identity_limit_and_counter_reset_refused(self):
        for change in [lambda r: r["events"].update(oom=1), lambda r: r["events"].update(oom_kill=1),
                       lambda r: r["events"].update(oom_group_kill=1), lambda r: r.update(identity="foreign"),
                       lambda r: r.update(memory_max=P.MEMORY_FLOOR), lambda r: r.update(peak=99),
                       lambda r: r["events"].update(low=-1)]:
            producer = P.Producer(self.root, "fixture", P.HOSTED_PROFILE); row = self.row("launcher")
            with patch.object(P, "cgroup_chain", return_value=[row]), patch.object(P, "resources", return_value=self.measured(P.MEMORY_FLOOR)):
                producer.check_resources("preflight"); change(row)
                with self.assertRaises(P.Refusal): producer.check_resources("python-validation", running=True)

    def test_bootstrap_absence_retains_full_commitment_not_live_cap_proof(self):
        self.producer.builder_created = True
        with patch.object(P, "cgroup_chain", return_value=[]), patch.object(self.producer, "command", return_value=b""), \
                patch.object(P, "resources", return_value=self.measured(P.BUILDER_MEMORY + P.HOST_RESERVE)):
            self.producer.check_resources("before-bootstrap")
            self.producer.check_resources("builder-bootstrap", running=True)
            self.assertNotIn("builder", self.producer.chains)
        with patch.object(P, "cgroup_chain", return_value=[]), patch.object(self.producer, "command", return_value=b""), \
                patch.object(P, "resources", return_value=self.measured(P.BUILDER_MEMORY + P.HOST_RESERVE-1)):
            with self.assertRaisesRegex(P.Refusal, "RESOURCE_NOTADMITTED"): self.producer.check_resources("builder-bootstrap", running=True)

    def test_each_phase_reserve_crossing_and_owned_termination_reap_closure(self):
        for phase in ["builder-bootstrap", "shipping-build", "image-save", "python-validation"]:
            with self.subTest(phase=phase):
                producer = P.Producer(self.root, "fixture", P.HOSTED_PROFILE)
                producer.builder_created = True; producer.cid = self.cid
                producer.builder_closed = phase in {"image-save", "python-validation"}
                leaf = self.row("builder", P.BUILDER_MEMORY)
                with patch.object(P, "cgroup_chain", side_effect=lambda pid="self", **kw: [] if pid == "self" else [leaf]), \
                        patch.object(producer, "owned_builder", return_value=self.live), patch.object(P, "resources", return_value=self.measured(P.HOST_RESERVE-1)):
                    with self.assertRaisesRegex(P.Refusal, "RESOURCE_NOTADMITTED"): producer.check_resources(phase, running=True)
        producer = P.Producer(self.root, "fixture", P.HOSTED_PROFILE)
        process = unittest.mock.Mock(pid=987, returncode=None)
        process.poll.return_value = None
        def reap(*args, **kwargs): process.returncode = -15; return -15
        process.wait.side_effect = reap
        with patch.object(P.subprocess, "Popen", return_value=process), patch.object(P.os, "killpg") as kill, \
                patch.object(P, "resources", return_value=self.measured(P.HOST_RESERVE-1)), patch.object(P, "cgroup_chain", return_value=[]):
            with self.assertRaisesRegex(P.Refusal, "RESOURCE_NOTADMITTED"): producer.command("image-save", ["owned-fixture"], monitored=True)
            kill.assert_called_once_with(987, P.signal.SIGTERM); process.wait.assert_called_once_with(timeout=30)
        self.assertEqual(producer.stages[-1]["exit"], -15)
        producer.builder_created = True; producer.cid = self.cid
        stopped = copy.deepcopy(self.live); stopped["State"].update(Running=False, Pid=0)
        with patch.object(producer, "owned_builder", side_effect=[self.live, stopped]), patch.object(producer, "command") as command, \
                patch.object(producer, "check_resources") as closing_guard:
            producer.stop_builder()
            self.assertTrue(producer.builder_closed)
            self.assertEqual(command.call_args.args[1], ["docker", "stop", self.cid])
            self.assertTrue(command.call_args.kwargs["monitored"]); self.assertEqual(closing_guard.call_count, 2)

    def test_guarded_plain_compressed_multilayer_validation_and_crossing(self):
        for compressed in [False, True]:
            p, image_id = layered_image_fixture(self.root, [("var/valid", b"ok", 0o600)], compressed=compressed)
            guard = unittest.mock.Mock()
            image = P.inspect_saved_image(p, image_id, scan_canary=True, guard=guard)
            self.assertEqual(set(image["binaries"]), P.BINARIES); self.assertGreater(guard.call_count, 10)
            with self.assertRaisesRegex(P.Refusal, "RESOURCE_NOTADMITTED"):
                P.inspect_saved_image(p, image_id, guard=unittest.mock.Mock(side_effect=P.Refusal("RESOURCE_NOTADMITTED")))
        guard = unittest.mock.Mock(side_effect=[None, P.Refusal("RESOURCE_NOTADMITTED")])
        with self.assertRaisesRegex(P.Refusal, "RESOURCE_NOTADMITTED"): P.hash_stream(io.BytesIO(bytes(2*1024**2)), guard=guard)
        artifact = ArtifactControls("test_exact_three_artifacts"); artifact.setUp()
        try:
            guard = unittest.mock.Mock()
            P.validate_public(artifact.root, artifact.receipt, artifact.inputs, guard=guard)
            self.assertGreater(guard.call_count, 10)
            with self.assertRaisesRegex(P.Refusal, "RESOURCE_NOTADMITTED"):
                P.validate_public(artifact.root, artifact.receipt, artifact.inputs, guard=unittest.mock.Mock(side_effect=P.Refusal("RESOURCE_NOTADMITTED")))
        finally: artifact.doCleanups()

    def test_profile_receipt_and_only_single_workflow_invocation_delta(self):
        import yaml
        receipt = self.producer.profile_receipt()
        self.assertEqual(receipt["name"], P.HOSTED_PROFILE); self.assertEqual(receipt["experimental_sufficiency"], "NOT_PROVEN")
        self.assertEqual((receipt["start_memory_floor"], receipt["running_memory_floor"]), (14*1024**3, 2*1024**3))
        current = yaml.safe_load((ROOT / ".github/workflows/install.yml").read_text())
        prior = yaml.safe_load(subprocess.check_output(["git", "-C", str(ROOT), "show", "dfc689ef70e5ff50ac39ae45a8cb4fcff465fb77:.github/workflows/install.yml"], text=True))
        command = current["jobs"]["shipping-image-producer"]["steps"][2]["run"]
        line = "  --resource-profile " + P.HOSTED_PROFILE + " \\\n"
        self.assertEqual(command.count(line), 1)
        current["jobs"]["shipping-image-producer"]["steps"][2]["run"] = command.replace(line, "")
        self.assertEqual(current, prior)

    def test_recipe_wires_all_guards_and_seals_latest_measurements(self):
        fixture = self.root / "fixture"; fixture.mkdir(); archive, image_id = image_fixture(fixture)
        work = self.root / "work"; work.mkdir(); producer = P.Producer(work, "fixture", P.HOSTED_PROFILE)
        product = self.root / "product"; tooling = self.root / "tooling"; commands = []
        inputs = {str(n): {} for n in range(2134)}; leaf = self.row("builder", P.BUILDER_MEMORY)
        live = copy.deepcopy(self.live)
        def command(stage, argv, monitored=False):
            commands.append((stage, argv, monitored))
            if monitored: producer.check_resources(stage, running=True)
            if stage == "docker-root": return str(work).encode()
            if stage.startswith("builder-discovery-"): return (self.cid + "\n").encode()
            if stage == "builder-id": return self.cid.encode()
            if stage == "builder-cgroups": return (str(P.BUILDER_MEMORY) + "\n200000 100000\n").encode()
            if stage == "builder-version": return b"buildkitd v0.26.0"
            if stage == "image-id": return image_id.encode()
            if stage == "image-save": Path(argv[3]).write_bytes(archive.read_bytes())
            if stage.startswith("builder-stop-"): live["State"].update(Running=False, Pid=0)
            return b""
        def measured(paths):
            # Validation changes the sampled minimum after the first encoded
            # receipt; verify the final artifact records that later measurement.
            return self.measured(P.HOST_RESERVE + (10 if "python-validation" in producer.phases else 100) if producer.builder_closed else P.MEMORY_FLOOR)
        with patch.object(P.platform, "system", return_value="Linux"), patch.object(P.platform, "machine", return_value="x86_64"), \
                patch.object(Path, "read_text", return_value="ID=ubuntu\nVERSION_ID=26.04\n"), \
                patch.object(P, "git", return_value=b"a"*40), patch.object(P, "checkout", side_effect=lambda root, *args: inputs if root == product else {"tool": {}}), \
                patch.object(P, "snapshot", side_effect=lambda root, context, rows: context.mkdir()), patch.object(P, "verify_snapshot"), \
                patch.object(P, "__file__", str(tooling / "scripts/selected-backend-ci/shipping-image-producer.py")), \
                patch.object(P, "cgroup_chain", side_effect=lambda pid="self", **kw: [] if pid == "self" or kw.get("relative") is not None else [leaf]), \
                patch.object(P, "resources", side_effect=measured), patch.object(producer, "owned_builder", return_value=live), \
                patch.object(producer, "command", side_effect=command):
            receipt = producer.build(product, tooling, "a"*40)
        self.assertEqual(set(receipt["resource_profile"]["phases"]), {"preflight", "before-bootstrap", "builder-bootstrap", "shipping-build",
            "before-builder-stop", "builder-closing", "builder-stop-0", "image-save", "python-validation", "complete"})
        self.assertEqual(receipt["minimum_sampled_resources"]["effective_mem_available"], P.HOST_RESERVE+10)
        self.assertEqual(receipt["resource_profile"]["name"], P.HOSTED_PROFILE)
        self.assertTrue(receipt["builder"]["stopped"])
        self.assertEqual(json.loads((work / "public/producer-receipt.json").read_bytes()), receipt)
        P.validate_public(work / "public", receipt, inputs)
        build = next(argv for stage, argv, _ in commands if stage == "shipping-build")
        self.assertIn("--load", build); self.assertIn("FVOCI_BUILD_SHA=" + P.PRODUCT_SHA, build)


class GlobalRootVisibilityControls(unittest.TestCase):
    setUp = HostedExperimentalResourceControls.setUp
    make_chain = HostedExperimentalResourceControls.make_chain
    measured = HostedExperimentalResourceControls.measured
    row = HostedExperimentalResourceControls.row

    def test_genuine_root_independent_of_unavailable_or_different_init_namespace(self):
        proc, root = self.make_chain()
        (proc / "1/ns/cgroup").unlink(); (proc / "1/ns/cgroup").symlink_to("different:[2]")
        with patch.object(P.os, "readlink", side_effect=AssertionError("NO_INIT_READLINK_OR_FALLBACK")):
            proof = {}; rows = P.cgroup_chain(root=root, proc=proc, proof=proof)
        self.assertEqual([r["path"] for r in rows], ["parent", "parent/child"])
        self.assertEqual(proof["pid"], [str(P.os.getpid()), 100, "parent/child"])
        self.assertEqual(len(proof["root"]), 64)

    def test_present_markers_and_disabled_controller_subtrees_refused(self):
        proc, root = self.make_chain()
        for marker in ["cgroup.type", "memory.max"]:
            (root / marker).write_text("domain" if marker == "cgroup.type" else "max")
            with self.assertRaisesRegex(P.Refusal, "CGROUP_HOST_ROOT_INVALID"): P.cgroup_chain(root=root, proc=proc)
            (root / marker).unlink()
        (root / "cgroup.controllers").write_text("cpu")
        with self.assertRaisesRegex(P.Refusal, "CGROUP_HOST_ROOT_INVALID"): P.cgroup_chain(root=root, proc=proc)

    def test_marker_permission_and_unknown_errors_never_become_absence(self):
        proc, root = self.make_chain(); original = P.os.open
        for marker in ["cgroup.type", "memory.max"]:
            for number in [13, 1, 20, 5, 116]:
                def denied(name, *a, **kw):
                    if name == marker and kw.get("dir_fd") is not None and P.os.fstat(kw["dir_fd"]).st_ino == root.stat().st_ino:
                        raise OSError(number, "PRIVATE_HOST_VALUE", marker)
                    return original(name, *a, **kw)
                with self.subTest(marker=marker, errno=number), patch.object(P.os, "open", side_effect=denied):
                    with self.assertRaisesRegex(P.Refusal, "CGROUP_METADATA_UNKNOWN"): P.cgroup_chain(root=root, proc=proc)

    def test_wrong_filesystem_and_foreign_leaf_refused_without_search(self):
        proc, root = self.make_chain()
        with patch.object(P, "cgroup_filesystem", side_effect=P.Refusal("CGROUP_HOST_ROOT_INVALID")):
            with self.assertRaisesRegex(P.Refusal, "CGROUP_HOST_ROOT_INVALID"): P.cgroup_chain(root=root, proc=proc)
        (root / "parent/child/cgroup.procs").write_text("42\n")
        with self.assertRaisesRegex(P.Refusal, "CGROUP_METADATA_UNKNOWN"): P.cgroup_chain(root=root, proc=proc)

    def test_mount_masks_stack_and_malformed_records_refused(self):
        proc, root = self.make_chain(); file = proc / "self/mountinfo"; original = file.read_text()
        for point in [root, root / "cgroup.type", root / "parent", root / "parent/child/memory.peak", proc / str(P.os.getpid()) / "stat"]:
            file.write_text(original + f"3 1 0:9 / {point} rw - tmpfs tmpfs rw\n")
            with self.subTest(point=point), self.assertRaisesRegex(P.Refusal, "CGROUP_ANCESTORS_HIDDEN"):
                P.cgroup_chain(root=root, proc=proc)
        for extra in ["malformed\n", "3 0 invalid / /unrelated rw - tmpfs tmpfs rw\n", "3 0 0:9 / /bad\\999 rw - tmpfs tmpfs rw\n"]:
            file.write_text(original + extra)
            with self.assertRaisesRegex(P.Refusal, "CGROUP_METADATA_UNKNOWN"): P.cgroup_chain(root=root, proc=proc)
        file.write_text(original + "3 0 0:9 / /unrelated\\040name rw - tmpfs tmpfs rw\n")
        self.assertEqual(len(P.cgroup_chain(root=root, proc=proc)), 2)

    def test_pid_start_path_or_membership_drift_during_sample_refused(self):
        proc, root = self.make_chain(); read = P.cgroup_text
        for target, replacement in [(proc / "self/stat", str(P.os.getpid()) + " (fixture) S " + "0 "*18 + "101 0\n"),
                                    (proc / "self/cgroup", "0::/parent\n"), (root / "parent/child/cgroup.procs", "42\n")]:
            old = target.read_bytes(); changed = False
            def changing(fd, name):
                nonlocal changed
                value = read(fd, name)
                if name == "memory.current" and not changed: target.write_text(replacement); changed = True
                return value
            with self.subTest(target=target.name), patch.object(P, "cgroup_text", side_effect=changing):
                with self.assertRaisesRegex(P.Refusal, "CGROUP_METADATA_UNKNOWN"): P.cgroup_chain(root=root, proc=proc)
            target.write_bytes(old)

    def test_stored_pid_root_and_directory_identity_refused(self):
        proc, root = self.make_chain(); proof = {}; P.cgroup_chain(root=root, proc=proc, proof=proof)
        original = proof.copy()
        for key, wrong in [("root", "foreign"), ("pid", [str(P.os.getpid()), 101, "parent/child"])]:
            proof.update(original); proof[key] = wrong
            with self.assertRaisesRegex(P.Refusal, "CGROUP_METADATA_UNKNOWN"): P.cgroup_chain(root=root, proc=proc, proof=proof)
        chain = P.cgroup_chain
        def sample(pid="self", **kw): return chain(pid, root=root, proc=proc, **kw)
        with patch.object(P, "cgroup_chain", side_effect=sample), patch.object(P, "resources", return_value=self.measured(P.MEMORY_FLOOR)):
            self.producer.check_resources("preflight")
            child = root / "parent/child"; child.rename(root / "parent/old-child"); child.mkdir()
            for f in (root / "parent/old-child").iterdir(): (child / f.name).write_bytes(f.read_bytes())
            with self.assertRaisesRegex(P.Refusal, "CGROUP_IDENTITY_DRIFT"): self.producer.check_resources("preflight")

    def test_symlink_threaded_deleted_and_bounded_membership_refused(self):
        proc, root = self.make_chain()
        for f in [root / "parent/child/memory.peak", root / "parent/child/cgroup.type"]:
            data = f.read_bytes(); f.unlink(); f.symlink_to(root / "parent" / f.name)
            with self.assertRaisesRegex(P.Refusal, "CGROUP_METADATA_UNKNOWN"): P.cgroup_chain(root=root, proc=proc)
            f.unlink(); f.write_bytes(data)
        child = root / "parent/child"; child.rename(root / "parent/held-child"); child.symlink_to(root / "parent/held-child")
        with self.assertRaisesRegex(P.Refusal, "CGROUP_METADATA_UNKNOWN"): P.cgroup_chain(root=root, proc=proc)
        child.unlink(); (root / "parent/held-child").rename(child)
        for value in ["threaded", "domain threaded", "domain invalid"]:
            (child / "cgroup.type").write_text(value)
            with self.assertRaisesRegex(P.Refusal, "CGROUP_METADATA_UNKNOWN"): P.cgroup_chain(root=root, proc=proc)
        (child / "cgroup.type").write_text("domain")
        for value in ["0::/parent/child (deleted)\n", "0::/parent/../child\n"]:
            (proc / "self/cgroup").write_text(value)
            with self.assertRaisesRegex(P.Refusal, "CGROUP_PATH_INVALID"): P.cgroup_chain(root=root, proc=proc)
        (proc / "self/cgroup").write_text("0::/parent/child\n")
        for value in ["42", "42\n"*32769]:
            (child / "cgroup.procs").write_text(value)
            with self.assertRaisesRegex(P.Refusal, "CGROUP_METADATA_UNKNOWN"): P.cgroup_chain(root=root, proc=proc)

    def test_closing_requires_bound_root_never_reads_stopped_leaf_or_pid(self):
        proc, root = self.make_chain(); proof = {}; before = P.cgroup_chain(root=root, proc=proc, proof=proof)
        with self.assertRaisesRegex(P.Refusal, "CGROUP_METADATA_UNKNOWN"): P.cgroup_chain(root=root, proc=proc, relative="parent")
        (proc / "self/cgroup").unlink(); (proc / "self/stat").unlink()
        for f in (root / "parent/child").iterdir(): f.unlink()
        after = P.cgroup_chain(root=root, proc=proc, relative="parent", proof=proof)
        self.assertEqual(after, before[:-1]); self.assertEqual(proof["pid"], [str(P.os.getpid()), 100, "parent/child"])

    def test_builder_reinspection_pid_oom_and_root_mismatch_refuse(self):
        for delta, code in [("pid", "BUILDER_IDENTITY_DRIFT"), ("oom", "BUILDER_OOM"), ("root", "CGROUP_IDENTITY_DRIFT")]:
            producer = P.Producer(self.root, "fixture", P.HOSTED_PROFILE); producer.cid = self.cid; producer.builder_created = True
            after = copy.deepcopy(self.live)
            if delta == "pid": after["State"]["Pid"] += 1
            if delta == "oom": after["State"]["OOMKilled"] = True
            def sample(pid="self", *, proof):
                proof["root"] = "launcher" if pid == "self" or delta != "root" else "foreign"
                return [] if pid == "self" else [self.row("builder", P.BUILDER_MEMORY)]
            with patch.object(P, "cgroup_chain", side_effect=sample), patch.object(P, "resources", return_value=self.measured(P.MEMORY_FLOOR)), \
                    patch.object(producer, "owned_builder", side_effect=[self.live, after]):
                with self.assertRaisesRegex(P.Refusal, code): producer.check_resources("shipping-build", running=True)

    def test_fixed_stat_argv_bounded_output_wait_and_error_closure(self):
        from types import SimpleNamespace
        info = SimpleNamespace(st_mode=0o100755, st_uid=0, st_dev=1, st_ino=2, st_size=100)
        def tool_stat(path): return info if path == Path("/usr/bin/stat") else SimpleNamespace(st_mode=0o40755, st_uid=0)
        for output, status, error in [(b"63677270\n", 0, None), (b"9fa0\n", 0, "CGROUP_HOST_ROOT_INVALID"),
                                      (b"63677270\n", 1, "CGROUP_HOST_ROOT_INVALID"), (b"x"*33, 0, "CGROUP_METADATA_UNKNOWN")]:
            child = unittest.mock.Mock(returncode=status); child.stdout.fileno.return_value = 987
            with patch.object(Path, "lstat", autospec=True, side_effect=tool_stat), patch.object(Path, "read_bytes", return_value=b"synthetic tool"), \
                    patch.object(P.subprocess, "Popen", return_value=child) as spawn, patch.object(P.select, "select", return_value=([child.stdout], [], [])), \
                    patch.object(P.os, "read", side_effect=[output, b""]):
                if error:
                    with self.assertRaisesRegex(P.Refusal, error): P.cgroup_filesystem(123, b"63677270")
                else: self.assertEqual(P.cgroup_filesystem(123, b"63677270"), P.digest(b"synthetic tool"))
            self.assertEqual(spawn.call_args.args[0], ["/usr/bin/stat", "--file-system", "--format=%t", "--", "/proc/self/fd/123"])
            self.assertEqual(spawn.call_args.kwargs["pass_fds"], (123,)); self.assertEqual(spawn.call_args.kwargs["env"], {"LC_ALL": "C", "PATH": "/usr/bin:/bin"})
            child.wait.assert_called(); child.stdout.close.assert_called_once()
        child = unittest.mock.Mock(returncode=None); child.poll.return_value = None
        with patch.object(Path, "lstat", autospec=True, side_effect=tool_stat), patch.object(Path, "read_bytes", return_value=b"synthetic tool"), \
                patch.object(P.subprocess, "Popen", return_value=child), patch.object(P.select, "select", return_value=([], [], [])):
            with self.assertRaisesRegex(P.Refusal, "CGROUP_METADATA_UNKNOWN"): P.cgroup_filesystem(123, b"63677270")
        child.kill.assert_called_once(); child.wait.assert_called_once_with(timeout=5); child.stdout.close.assert_called_once()

    def test_mount_and_root_replacement_drift_and_all_owned_fds_closed(self):
        proc, root = self.make_chain(); file = proc / "self/mountinfo"; original = file.read_text(); read = P.cgroup_text
        opened = []; os_open = P.os.open; changed = False
        def recording(*a, **kw):
            fd = os_open(*a, **kw); opened.append(fd); return fd
        def changing(fd, name):
            nonlocal changed
            value = read(fd, name)
            if name == "memory.current" and not changed:
                file.write_text(original + "3 0 0:9 / /unrelated rw - tmpfs tmpfs rw\n"); changed = True
            return value
        with patch.object(P.os, "open", side_effect=recording), patch.object(P, "cgroup_text", side_effect=changing):
            with self.assertRaisesRegex(P.Refusal, "CGROUP_METADATA_UNKNOWN"): P.cgroup_chain(root=root, proc=proc)
        for fd in opened:
            with self.assertRaises(OSError): P.os.fstat(fd)
        file.write_text(original); proof = {}; P.cgroup_chain(root=root, proc=proc, proof=proof)
        root.rename(self.root / "held-root"); root.mkdir()
        for source in (self.root / "held-root").rglob("*"):
            destination = root / source.relative_to(self.root / "held-root")
            if source.is_dir(): destination.mkdir()
            else: destination.write_bytes(source.read_bytes())
        # Filesystem evidence remains synthetic and inode-authentication changes.
        with self.assertRaisesRegex(P.Refusal, "CGROUP_METADATA_UNKNOWN"): P.cgroup_chain(root=root, proc=proc, proof=proof)

    def test_stat_untrusted_tool_or_parent_and_changed_bytes_refused(self):
        from types import SimpleNamespace
        base = dict(st_mode=0o100755, st_uid=0, st_dev=1, st_ino=2, st_size=100)
        for delta in [dict(st_uid=1000), dict(st_mode=0o100777), dict(st_mode=0o120777), dict(st_size=1024**2+1)]:
            info = SimpleNamespace(**{**base, **delta})
            def metadata(path): return info if path == Path("/usr/bin/stat") else SimpleNamespace(st_mode=0o40755, st_uid=0)
            with patch.object(Path, "lstat", autospec=True, side_effect=metadata), patch.object(P.subprocess, "Popen") as spawn:
                with self.assertRaisesRegex(P.Refusal, "CGROUP_METADATA_UNKNOWN"): P.cgroup_filesystem(123, b"63677270")
            spawn.assert_not_called()
        with patch.object(Path, "lstat", return_value=SimpleNamespace(st_mode=0o40777, st_uid=0)), patch.object(P.subprocess, "Popen") as spawn:
            with self.assertRaisesRegex(P.Refusal, "CGROUP_METADATA_UNKNOWN"): P.cgroup_filesystem(123, b"63677270")
        spawn.assert_not_called()
        def metadata(path): return SimpleNamespace(**base) if path == Path("/usr/bin/stat") else SimpleNamespace(st_mode=0o40755, st_uid=0)
        child = unittest.mock.Mock(returncode=0)
        with patch.object(Path, "lstat", autospec=True, side_effect=metadata), patch.object(Path, "read_bytes", side_effect=[b"original", b"changed"]), \
                patch.object(P.subprocess, "Popen", return_value=child), patch.object(P.select, "select", return_value=([child.stdout], [], [])), \
                patch.object(P.os, "read", side_effect=[b"63677270\n", b""]):
            with self.assertRaisesRegex(P.Refusal, "CGROUP_METADATA_UNKNOWN"): P.cgroup_filesystem(123, b"63677270")
        child.wait.assert_called(); child.stdout.close.assert_called_once()


class ClosingPhaseControls(unittest.TestCase):
    setUp = HostedExperimentalResourceControls.setUp
    row = HostedExperimentalResourceControls.row
    measured = HostedExperimentalResourceControls.measured

    def closure(self, terminal=False, crossing=None, *, run_export=True):
        producer = self.producer; producer.builder_created = True; producer.cid = self.cid
        live = copy.deepcopy(self.live); parent = self.row("parent", P.MEMORY_FLOOR, 100)
        leaf = self.row("builder", P.BUILDER_MEMORY, 100); commands = []; stopping = False
        def chain(pid="self", *, relative=None, **kw):
            if relative is not None:
                self.assertEqual(relative, "parent"); return [parent]
            return [] if pid == "self" else [parent, leaf]
        def measured(paths):
            reserve = P.HOST_RESERVE
            if stopping and crossing in {"host", "terminal-host"} and (crossing == "host" or not live["State"]["Running"]): reserve -= 1
            if stopping and crossing in {"ancestor", "terminal-ancestor"} and (crossing == "ancestor" or not live["State"]["Running"]): parent["current"] = parent["memory_max"] - P.HOST_RESERVE + 1; parent["peak"] = parent["current"]
            if stopping and crossing == "ancestor-oom": parent["events"]["oom"] = 1
            return self.measured(reserve)
        def command(stage, argv, monitored=False):
            nonlocal stopping
            commands.append((stage, argv, monitored))
            if argv[:2] == ["docker", "inspect"]: return P.encoded([live])
            self.assertEqual(argv, ["docker", "stop", self.cid])
            stopping = True
            if crossing and monitored: producer.check_resources(stage, running=True)
            live["State"].update(Running=False, Pid=0)
            if terminal == "MISSING": live["State"].pop("OOMKilled")
            else: live["State"]["OOMKilled"] = terminal
            if monitored: producer.check_resources(stage, running=True)
            return b""
        with patch.object(P, "cgroup_chain", side_effect=chain), patch.object(P, "resources", side_effect=measured), \
                patch.object(producer, "command", side_effect=command):
            producer.check_resources("before-builder-stop", running=True)
            producer.stop_builder()
            if run_export: producer.check_resources("image-save", running=True)
        return commands

    def test_terminal_oom_during_stop_refused_before_closed_export(self):
        with self.assertRaisesRegex(P.Refusal, "BUILDER_OOM"): self.closure(True)
        self.assertFalse(self.producer.builder_closed)
        self.assertNotIn("image-save", self.producer.phases)

    def test_terminal_missing_or_illtyped_oom_refused(self):
        for terminal in ["MISSING", None, 0, 1, "false", "true", {}, []]:
            with self.subTest(terminal=terminal):
                self.producer = P.Producer(self.root, "fixture", P.HOSTED_PROFILE)
                with self.assertRaisesRegex(P.Refusal, "BUILDER_METADATA_UNKNOWN"): self.closure(terminal)
                self.assertFalse(self.producer.builder_closed)

    def test_closing_host_and_ancestor_reserve_or_oom_failure_refused(self):
        for crossing, error in [("host", "RESOURCE_NOTADMITTED"), ("ancestor", "RESOURCE_NOTADMITTED"), ("ancestor-oom", "CGROUP_OOM_INCREMENT"), ("terminal-host", "RESOURCE_NOTADMITTED"), ("terminal-ancestor", "RESOURCE_NOTADMITTED")]:
            with self.subTest(crossing=crossing):
                self.producer = P.Producer(self.root, "fixture", P.HOSTED_PROFILE)
                with self.assertRaisesRegex(P.Refusal, error): self.closure(crossing=crossing)
                self.assertFalse(self.producer.builder_closed)

    def test_healthy_terminal_closure_preserves_last_counters_final_unknown(self):
        commands = self.closure()
        self.assertTrue(self.producer.builder_closed); self.assertFalse(self.producer.builder_closing)
        self.assertTrue(next(monitored for stage, _, monitored in commands if stage.startswith("builder-stop-")))
        receipt = self.producer.profile_receipt()
        self.assertEqual(receipt["builder_terminal"], {"oom_killed": False, "cgroup_counters": "UNKNOWN"})
        self.assertEqual(receipt["cgroups"]["builder"]["current"], 100)
        self.assertEqual(receipt["chains"]["builder-ancestors"], ["parent"])
        self.assertEqual(receipt["phases"]["builder-closing"]["memory_floor"], P.HOST_RESERVE)

    def test_recorded_parent_reread_without_leaf_pid_and_identity_refusal(self):
        proc, root = HostedExperimentalResourceControls.make_chain(self)
        with patch.object(P.os, "readlink", side_effect=lambda p: "cgroup:[1]"):
            proof = {}; before = P.cgroup_chain(root=root, proc=proc, proof=proof)
            (proc / "self/cgroup").unlink()
            parent = root / "parent"
            (parent / "memory.current").write_text("150")
            after = P.cgroup_chain(root=root, proc=proc, relative="parent", proof=proof)
            self.assertEqual(after[0]["identity"], before[0]["identity"])
            self.assertEqual(after[0]["current"], 150)
            self.assertEqual(after[0]["path"], "parent")
            with self.assertRaisesRegex(P.Refusal, "CGROUP_PATH_INVALID"): P.cgroup_chain(root=root, proc=proc, relative="../parent", proof=proof)
        producer = self.producer; producer.builder_created = True; producer.cid = self.cid
        producer.builder_closing = True; producer.builder_parent = "parent"; producer.chains["builder"] = ["original", "builder"]
        stopped = copy.deepcopy(self.live); stopped["State"].update(Running=False, Pid=0)
        with patch.object(producer, "owned_builder", return_value=stopped), patch.object(P, "resources", return_value=self.measured(P.HOST_RESERVE)), \
                patch.object(P, "cgroup_chain", side_effect=lambda pid="self", **kw: [self.row("foreign")] if kw.get("relative") is not None else []):
            with self.assertRaisesRegex(P.Refusal, "CGROUP_IDENTITY_DRIFT"): producer.check_resources("builder-closing", running=True)

    def test_actual_stop_command_wait_and_failure_terminate_reap(self):
        producer = self.producer; producer.builder_created = True; producer.cid = self.cid
        stopped = copy.deepcopy(self.live); stopped["State"].update(Running=False, Pid=0)
        for fails in [False, True]:
            producer.builder_closed = False
            process = unittest.mock.Mock(pid=987, returncode=None)
            process.poll.side_effect = [None, None] if fails else [None, 0]
            def reap(*args, **kwargs): process.returncode = -15 if fails else 0; return process.returncode
            process.wait.side_effect = reap
            with patch.object(producer, "owned_builder", side_effect=[self.live, stopped] if not fails else [self.live]), \
                    patch.object(producer, "check_resources", side_effect=[None, P.Refusal("RESOURCE_NOTADMITTED")] if fails else None), \
                    patch.object(P.subprocess, "Popen", return_value=process) as popen, patch.object(P.os, "killpg") as kill:
                if fails:
                    with self.assertRaisesRegex(P.Refusal, "RESOURCE_NOTADMITTED"): producer.stop_builder()
                    kill.assert_called_once_with(987, P.signal.SIGTERM); process.wait.assert_called_once_with(timeout=30)
                    self.assertFalse(producer.builder_closed)
                else:
                    producer.stop_builder(); self.assertTrue(producer.builder_closed); kill.assert_not_called()
                    process.wait.assert_called_once_with(timeout=1)
                self.assertEqual(popen.call_args.args[0], ["docker", "stop", self.cid])
                self.assertTrue(popen.call_args.kwargs["start_new_session"])

    def test_first_failure_priority_survives_closure_oom(self):
        # main owns its process umask; restore it in this in-process fixture.
        old_umask = P.os.umask(0o022); P.os.umask(old_umask); self.addCleanup(P.os.umask, old_umask)
        producer = self.producer
        def fail_build(*args): producer.builder_created = True; raise P.Refusal("ORIGINAL_FAILURE")
        stderr = io.StringIO()
        with patch.object(P, "Producer", return_value=producer), patch.object(producer, "build", side_effect=fail_build), \
                patch.object(producer, "stop_builder", side_effect=P.Refusal("BUILDER_OOM")), patch.object(P.sys, "stderr", stderr), \
                patch.object(P.signal, "signal"):
            result = P.main(["--product-root", str(self.root / "product"), "--tooling-root", str(self.root / "tooling"),
                "--tooling-sha", "a"*40, "--work-dir", str(self.root / "work"), "--run-id", "1-1", "--resource-profile", P.HOSTED_PROFILE])
        receipt = json.loads(stderr.getvalue())
        self.assertEqual(result, 1); self.assertEqual(receipt["result"], "ORIGINAL_FAILURE"); self.assertEqual(receipt["closure"], "BUILDER_OOM")


if __name__ == "__main__":
    unittest.main()
