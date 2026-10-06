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


if __name__ == "__main__":
    unittest.main()
