#!/bin/bash
set -e

BUILDER_IMAGE="heroku/builder:22"

echo "Pulling builder image $BUILDER_IMAGE..."
docker pull $BUILDER_IMAGE

echo "Creating temporary container..."
ID=$(docker create $BUILDER_IMAGE)

echo "Extracting /cnb..."
rm -rf builder-data
mkdir -p builder-data
docker cp $ID:/cnb ./builder-data/

echo "Cleaning up..."
docker rm -v $ID

echo "Builder data extracted to ./builder-data"
