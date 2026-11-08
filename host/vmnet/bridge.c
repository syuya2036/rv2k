// Ethernet-only vmnet <-> loopback UDP transport. No protocol responses here.
#include <vmnet/vmnet.h>
#include <dispatch/dispatch.h>
#include <xpc/xpc.h>
#include <uuid/uuid.h>
#include <arpa/inet.h>
#include <sys/socket.h>
#include <sys/uio.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>

static volatile sig_atomic_t interrupted;
static void stop(int sig) { (void)sig; interrupted = 1; }
static double seconds(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double)t.tv_sec + (double)t.tv_nsec / 1e9;
}
int main(int argc, char **argv) {
    if (argc != 2) { fprintf(stderr, "usage: bridge SECONDS (1..300)\n"); return 2; }
    char *end;
    long duration = strtol(argv[1], &end, 10);
    if (*end || duration < 1 || duration > 300) return 2;
    signal(SIGINT, stop);
    signal(SIGTERM, stop);
    int sock = socket(AF_INET, SOCK_DGRAM, 0);
    struct sockaddr_in local = {.sin_len=sizeof(local), .sin_family=AF_INET,
        .sin_port=htons(44001), .sin_addr={.s_addr=htonl(INADDR_LOOPBACK)}};
    struct sockaddr_in peer = local;
    peer.sin_port = htons(44000);
    if (sock < 0 || bind(sock, (struct sockaddr *)&local, sizeof(local)) ||
        connect(sock, (struct sockaddr *)&peer, sizeof(peer)) || fcntl(sock, F_SETFL, O_NONBLOCK)) {
        perror("loopback UDP"); return 2;
    }
    dispatch_queue_t queue = dispatch_queue_create("rv2k.vmnet.bridge", DISPATCH_QUEUE_SERIAL);
    dispatch_semaphore_t ready = dispatch_semaphore_create(0);
    xpc_object_t config = xpc_dictionary_create(NULL, NULL, 0);
    uuid_t network;
    uuid_parse("5f90c6e8-2cd1-4daf-b152-d2157222a32c", network);
    xpc_dictionary_set_uint64(config, vmnet_operation_mode_key, VMNET_HOST_MODE);
    xpc_dictionary_set_uuid(config, vmnet_network_identifier_key, network);
    xpc_dictionary_set_string(config, vmnet_host_ip_address_key, "192.168.123.1");
    xpc_dictionary_set_string(config, vmnet_host_subnet_mask_key, "255.255.255.0");
    xpc_dictionary_set_bool(config, vmnet_allocate_mac_address_key, false);
    xpc_dictionary_set_bool(config, vmnet_enable_tso_key, false);
    xpc_dictionary_set_bool(config, vmnet_enable_checksum_offload_key, false);
    __block vmnet_return_t status = VMNET_FAILURE;
    __block uint64_t max_packet = 0;
    interface_ref nic = vmnet_start_interface(config, queue, ^(vmnet_return_t result, xpc_object_t params) {
        status = result;
        if (params) max_packet = xpc_dictionary_get_uint64(params, vmnet_max_packet_size_key);
        dispatch_semaphore_signal(ready);
    });
    xpc_release(config);
    if (!nic || dispatch_semaphore_wait(ready, dispatch_time(DISPATCH_TIME_NOW, 5*NSEC_PER_SEC))) {
        fprintf(stderr, "vmnet start failed or timed out\n"); close(sock); return 3;
    }
    if (status != VMNET_SUCCESS) {
        printf("{\"vmnet_status\":%u,\"success\":false}\n", status);
        close(sock); return 3;
    }
    int result = 0;
    size_t to_guest = 0, to_host = 0, dropped = 0;
    // vmnet requires a buffer at least max_packet large, even with MTU=1500.
    uint8_t *buffer = max_packet && max_packet <= 65536 ? malloc((size_t)max_packet) : NULL;
    if (!buffer) { fprintf(stderr, "invalid vmnet packet size: %llu\n", max_packet); result = 4; }
    printf("{\"vmnet_status\":%u,\"success\":true,\"host\":\"192.168.123.1\",\"guest\":\"192.168.123.50\",\"max_packet\":%llu}\n", status, max_packet);
    fflush(stdout);
    double deadline = seconds() + duration;
    while (!result && !interrupted && seconds() < deadline) {
        struct iovec iov = {.iov_base=buffer, .iov_len=(size_t)max_packet};
        struct vmpktdesc packet = {.vm_pkt_size=(size_t)max_packet, .vm_pkt_iov=&iov, .vm_pkt_iovcnt=1};
        int count = 1;
        status = vmnet_read(nic, &packet, &count);
        if (status != VMNET_SUCCESS) { fprintf(stderr, "vmnet read: %u\n", status); result=5; break; }
        if (count) {
            if (packet.vm_pkt_size >= 14 && packet.vm_pkt_size <= 1514) {
                ssize_t sent = send(sock, buffer, packet.vm_pkt_size, 0);
                if (sent == (ssize_t)packet.vm_pkt_size) ++to_guest;
                else if (errno == ECONNREFUSED || errno == ENOBUFS) ++dropped;
                else { perror("UDP send"); result=5; break; }
            } else ++dropped;
        }
        // One bounded datagram per iteration, with a byte beyond the frame limit
        // so oversized/truncated frames cannot be accepted as valid Ethernet.
        uint8_t outbound[1515];
        ssize_t len = recv(sock, outbound, sizeof(outbound), 0);
        if (len >= 14 && len <= 1514) {
            struct iovec out_iov = {.iov_base=outbound, .iov_len=(size_t)len};
            struct vmpktdesc out = {.vm_pkt_size=(size_t)len, .vm_pkt_iov=&out_iov, .vm_pkt_iovcnt=1};
            count = 1;
            status = vmnet_write(nic, &out, &count);
            if (status != VMNET_SUCCESS || count != 1) { fprintf(stderr, "vmnet write: %u\n", status); result=5; break; }
            ++to_host;
        } else if (len >= 0) ++dropped;
        else if (errno != EAGAIN && errno != ECONNREFUSED) { perror("UDP recv"); result=5; break; }
        struct timespec pause = {.tv_nsec=1000000};
        nanosleep(&pause, NULL);
    }
    free(buffer);
    dispatch_semaphore_t stopped = dispatch_semaphore_create(0);
    __block vmnet_return_t stop_status = VMNET_FAILURE;
    vmnet_return_t request = vmnet_stop_interface(nic, queue, ^(vmnet_return_t s) {
        stop_status=s; dispatch_semaphore_signal(stopped);
    });
    if (request != VMNET_SUCCESS || dispatch_semaphore_wait(stopped, dispatch_time(DISPATCH_TIME_NOW, 5*NSEC_PER_SEC)) || stop_status != VMNET_SUCCESS) {
        fprintf(stderr, "vmnet stop failed\n"); result=6;
    }
    close(sock);
    printf("{\"to_guest\":%zu,\"to_host\":%zu,\"dropped\":%zu,\"stop_status\":%u}\n", to_guest, to_host, dropped, stop_status);
    return result;
}
