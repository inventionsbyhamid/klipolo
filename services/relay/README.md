# Klipolo relay

## Update the pinned packages

The image installs exact versions from two lock files, with `--no-deps`:

- `requirements-torch.lock`: torch, torchaudio, numpy and what they need, from the PyTorch CPU index.
- `requirements.lock`: everything else, from PyPI.

The image only installs these pins, so `pip freeze` inside it gives the same list back. To pick up new versions, resolve the unpinned requirements in a clean container instead. Run this in `services/relay`:

```sh
docker run --rm --cpus 4 --memory 4g -v "$PWD:/src:ro" python:3.12-slim-bookworm sh -c '
  pip install -q --root-user-action=ignore --index-url https://download.pytorch.org/whl/cpu \
    torch==2.8.0 torchaudio==2.8.0 numpy &&
  pip freeze --exclude pip > /tmp/torch.lock &&
  pip install -q --root-user-action=ignore /src &&
  pip freeze --exclude pip | grep -v "^klipolo-relay" | grep -vxF -f /tmp/torch.lock > /tmp/rest.lock;
  echo "== requirements-torch.lock"; cat /tmp/torch.lock; echo "== requirements.lock"; cat /tmp/rest.lock'
```

Copy each list under the comment header of its lock file, rebuild the image, and run the full test suite in it (see Test). torch and torchaudio stay at 2.8: it is the last release with `forced_align`.

## Move the base image

The Dockerfile pins `python:3.12-slim-bookworm` by its multi-platform index digest. To move to a newer build of the same tag:

```sh
docker buildx imagetools inspect python:3.12-slim-bookworm | grep -m1 Digest
```

Put that digest after `@sha256:` in the `FROM` line, rebuild, and run the full test suite.
