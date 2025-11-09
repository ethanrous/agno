all: amd64 arm64

amd64:
	docker build -f build/Dockerfile --platform linux/amd64 --build-arg TARGETARCH=amd64 --output type=local,dest=libagno-amd64.a .

arm64:
	docker build -f build/Dockerfile --platform linux/amd64 --build-arg TARGETARCH=arm64 --output type=local,dest=libagno-arm64.a .
