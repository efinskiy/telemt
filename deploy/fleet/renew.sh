#!/bin/sh
set -eu
cd "$(dirname "$0")"
docker compose -f compose.yml --profile certificate run --rm certbot renew --webroot -w /var/www/acme
docker compose -f compose.yml exec -T nginx nginx -t
docker compose -f compose.yml exec -T nginx nginx -s reload
