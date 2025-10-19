
# LinkSeeker, NAT punching one way or another

# Utilities

LinkSeeker has two utilities:

* Parse/Serialize UDP NAT Punch requests, and track requesters.
* Act as a pseudo-TURN server, meaning basically that UDP packets will "proxy" through this

## Ways of using

This crate, LinkSeeker has 2 ways of being used:

* Executable, which acts as a standalone server.
* Library, which is exepcted to be used by client using this protocol

There is no wrapping of a UDP socket in this library, it is assumed you have your own system and can filter
directly the udpunch messages from your framework.

# How it works

All udpunch messages start with "#lnksk@". Every UDP that has those 5 characters can be considered to be owned by this
library. Everything else is ignored or passed-through depending on the mode.

More precisely each message has this structure `#lnksk@$REQUEST_ID/key1=value1/key2=value2/key3=value3`.

For instance, a request to connect message would be like this:

`#lnksk@register`.

## Message types

Three messages types are used in this library

* PunchCheck
    * Checks if the NAT type is compatbile with NAT punching. Symmetrical NAT cannot do UDP NAT punching (https://www.checkmynat.com/)
    and we have a simple check to check that.
* RegisterLink: register a link ID with this crate, that can be communicated to someone else.
    * If the registerer is Punch compatible, send an ID given by this server
    * If the registerer is NOT Punch compatible, send an error
* RequestLink: request to join an ID, immediatly answers a punch order if the ID exists.
* Proxy: proxies request to a specific IP:port
* PunchOrder: order to punch with UDP a specific remote, to connect to a specific person. Sent by the server.