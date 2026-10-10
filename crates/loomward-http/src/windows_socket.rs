use std::io;
use std::os::windows::io::AsRawSocket;

// socket2 exposes bind/listen but not SO_EXCLUSIVEADDRUSE; set it before bind (Winsock2.h).
pub(super) fn exclusive_address_use(socket: &socket2::Socket) -> io::Result<()> {
    #[link(name = "Ws2_32")]
    extern "system" {
        fn setsockopt(socket: usize, level: i32, name: i32, value: *const u8, len: i32) -> i32;
        fn WSAGetLastError() -> i32;
    }
    const SOL_SOCKET: i32 = 0xffff;
    const SO_EXCLUSIVEADDRUSE: i32 = !0x0004;
    let enabled: i32 = 1;
    // SAFETY: the socket is live, and the value pointer covers len bytes for this synchronous call.
    let result = unsafe {
        setsockopt(
            socket.as_raw_socket() as usize,
            SOL_SOCKET,
            SO_EXCLUSIVEADDRUSE,
            (&enabled as *const i32).cast(),
            std::mem::size_of_val(&enabled) as i32,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        // SAFETY: WSAGetLastError has no preconditions and is read immediately after failure.
        Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listener_explicitly_sets_exclusive_address_use() {
        #[link(name = "Ws2_32")]
        extern "system" {
            fn getsockopt(
                socket: usize,
                level: i32,
                name: i32,
                value: *mut u8,
                len: *mut i32,
            ) -> i32;
        }
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .unwrap();
        exclusive_address_use(&socket).unwrap();
        let mut enabled: i32 = 0;
        let mut len = std::mem::size_of_val(&enabled) as i32;
        // SAFETY: both output pointers cover their advertised lengths and the socket is live.
        let result = unsafe {
            getsockopt(
                socket.as_raw_socket() as usize,
                0xffff,
                !0x0004,
                (&mut enabled as *mut i32).cast(),
                &mut len,
            )
        };
        assert_eq!(result, 0);
        assert_eq!(enabled, 1, "SO_EXCLUSIVEADDRUSE was not enabled");
        socket
            .bind(&std::net::SocketAddr::from(([127, 0, 0, 1], 0)).into())
            .unwrap();
        socket.listen(128).unwrap();
        let competing = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .unwrap();
        competing.set_reuse_address(true).unwrap();
        assert!(competing.bind(&socket.local_addr().unwrap()).is_err());
    }
}
