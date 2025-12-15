# DockerImage Updater

## tl;dr

The tool allows to get a new version for a given docker image (or Dockerfile(s)). There are 5 different strategies: Update to next minor version, update to latest minor version, update to next major version and update to lastest major version, or latest available ( a combination of the latest minor and major).

## Examples

```bash
dockerimage-updater input mcr.microsoft.com/dotnet/aspnet:9.0.0 --strat next-minor -q
mcr.microsoft.com/dotnet/aspnet:9.0.1

dockerimage-updater input bitnami/openldap:2.6.8-debian-12-r1 --strat latest-minor -q
bitnami/openldap:2.6.9-debian-12-r10

dockerimage-updater input node:18.0.0-alpine --strat next-major -q
node:19.2.0-alpine3.17

dockerimage-updater input node:18.0.0-alpine --strat latest-major -q
node:23.9.0-alpine3.21

dockerimage-updater input node:22.6.0-bookworm-slim -q
node:23.9.0-bookworm-slim

```

## Features

* The tool tries to keep the variant (e.g. alpine) in place and respects this during the update.
* The tool operates on semver tags only. Major and minor have to be already given. See example dockerfile.
* Cache files will be saved in the location of the binary, to reduce unncessary traffic (cache will be updated after an hour).
* Support for Dockerhub and Microsoft Container Registry (MCR)
* Quiet-mode only prints the result, in case the output need to be captured.
* Updating entire dockerfile(s) via file input. Dry-run can be used for a preview.
* Help available via: `dockerimage-updater --help`.

## Notes

* Filtering by architecture (e.g. "amd64") will be done on the initial fetch, when creating the cache. The cache file does not contain information about the architecture, and may lead to incorrect results. This should only be used when working with non-amd64 images, where the common tags might not exist.
