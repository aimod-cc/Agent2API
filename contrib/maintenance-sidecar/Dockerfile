FROM python:3.12-slim-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends libproxychains4 ca-certificates && rm -rf /var/lib/apt/lists/*
RUN pip install --no-cache-dir cryptography==44.0.3
WORKDIR /app
COPY *.py *.js *.css *.html vendor-manifest.json /app/
COPY vendor/ /app/vendor/
ENV PYTHONUNBUFFERED=1 PYTHONDONTWRITEBYTECODE=1 TZ=Asia/Shanghai
EXPOSE 8080
CMD ["python", "/app/server.py"]
