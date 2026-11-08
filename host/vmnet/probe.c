#include <vmnet/vmnet.h>
#include <dispatch/dispatch.h>
#include <xpc/xpc.h>
#include <stdio.h>
#include <errno.h>

int main(void) {
    dispatch_queue_t queue=dispatch_queue_create("rv2k.vmnet.probe",DISPATCH_QUEUE_SERIAL);
    dispatch_semaphore_t ready=dispatch_semaphore_create(0);
    xpc_object_t config=xpc_dictionary_create(NULL,NULL,0);
    xpc_dictionary_set_uint64(config,vmnet_operation_mode_key,VMNET_HOST_MODE);
    __block vmnet_return_t result=VMNET_FAILURE;
    interface_ref nic=vmnet_start_interface(config,queue,^(vmnet_return_t status,xpc_object_t params){
        (void)params;result=status;dispatch_semaphore_signal(ready);
    });
    if (!nic) { printf("{\"handle\":false,\"errno\":%d}\n",errno); return 2; }
    if(dispatch_semaphore_wait(ready,dispatch_time(DISPATCH_TIME_NOW,5*NSEC_PER_SEC))) {
        puts("{\"handle\":true,\"timeout\":true}");return 3;
    }
    printf("{\"handle\":true,\"vmnet_status\":%u,\"success\":%s}\n",result,result==VMNET_SUCCESS?"true":"false");
    if(result==VMNET_SUCCESS){
        dispatch_semaphore_t stopped=dispatch_semaphore_create(0);
        vmnet_stop_interface(nic,queue,^(vmnet_return_t status){(void)status;dispatch_semaphore_signal(stopped);});
        if(dispatch_semaphore_wait(stopped,dispatch_time(DISPATCH_TIME_NOW,5*NSEC_PER_SEC)))return 4;
    }
    return result==VMNET_SUCCESS?0:1;
}
