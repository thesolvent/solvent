// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

enum ProofKind {
    Fill,
    DirectRepayment
}

struct ProofEnvelope {
    uint8 version;
    ProofKind kind;
    bytes payload;
}

library ProofHashLib {
    uint8 internal constant VERSION = 1;

    function encode(ProofKind kind, bytes memory payload) internal pure returns (bytes memory) {
        return abi.encode(ProofEnvelope({ version: VERSION, kind: kind, payload: payload }));
    }
}
