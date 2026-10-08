FROM alpine:3.20

RUN apk add --no-cache tor

COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

# Keys are NOT copied in here - they are mounted as a volume at run time.
# A secret that lands in an image layer spreads to everyone who pulls the image
# and stays in the registry history even after "deletion".

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
