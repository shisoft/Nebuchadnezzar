(ns neb.test.durability
  (:require [midje.sweet :refer :all]
            [neb.core :refer :all]
            [neb.server :refer :all]
            [neb.durability.serv.core :refer [recover-backup list-backup-ids]]
            [neb.trunk-store :as ts :refer [backup-trunks]]
            [neb.cell :as cell]
            [cluster-connector.utils.for-debug :refer [$ spy]])
  (:import (org.shisoft.neb.utils StandaloneZookeeper)
           (java.io File)
           (org.apache.commons.io FileUtils)
           (org.shisoft.neb Trunk)
           (java.util Map$Entry)))

(def trunks-size (* Integer/MAX_VALUE 0.2))
(def memory-size (* Integer/MAX_VALUE 4))

(let [zk (StandaloneZookeeper.)
      config {:server-name :test-server
              :port 5134
              :zk  "127.0.0.1:21817"
              :trunks-size trunks-size
              :memory-size memory-size
              :data-path   "data"
              :durability true
              :auto-backsync false
              :replication 2}
      inserted-cell-ids (atom #{})
      placr-holder "Lorem Ipsum is simply dummy text of the printing and typesetting industry. Lorem Ipsum has been the industry's standard dummy text ever since the 1500s, when an unknown printer took a galley of type and scrambled it to make a type specimen book. It has survived not only five centuries, but also the leap into electronic typesetting, remaining essentially unchanged. It was popularised in the 1960s with the release of Letraset sheets containing Lorem Ipsum passages, and more recently with desktop publishing software like Aldus PageMaker including versions of Lorem Ipsum."]
  (.startZookeeper zk 21817)
  (try
    (facts "Durability"
           (fact "Start Server"
                 (start-server config) => anything)
           (fact "Prepare schemas"
                 (add-schema :raw-data [[:data :obj]]) => 0)
           (fact "Write something"
                 (dorun
                   (pmap
                     (fn [id]
                       (let [cell-id (new-cell (str "test" id)
                                               :raw-data {:data {:str (str placr-holder id)}})]
                         (swap! inserted-cell-ids conj cell-id)
                         cell-id => anything))
                     (range 300))))
           (swap! inserted-cell-ids sort)
           (fact "Check Recover"
                 (dorun
                   (map
                     (fn [id]
                       (let [str-key (str "test" id)]
                         (read-cell str-key) => (contains {:data {:str (str placr-holder id)}
                                                           :*id* (to-id str-key)})))
                     (range 300))))
           (fact "Check dirty"
                 (let [trunks (.getTrunks ts/trunks)]
                   (doseq [trunk trunks]
                     (let [dirtyRanges (.getDirtyRanges ^Trunk trunk)]
                       (.size dirtyRanges) => 1
                       (.getValue ^Map$Entry (first dirtyRanges)) => (dec (.getAppendHeaderValue trunk))))))
           (fact "Sync trunks"
                 (backup-trunks) => anything)
           (Thread/sleep 10000)
           (fact "Check backedup ids"
                 (sort (set (list-backup-ids))) => (just @inserted-cell-ids :gaps-ok :in-any-order))
           (fact "Delete Everything"
                 (dorun
                   (pmap
                     (fn [id]
                       (delete-cell (str "test" id)) => anything)
                     (range 300))))
           (fact "Check Deleted"
                 (let [trunks (.getTrunks ts/trunks)]
                   (doseq [trunk trunks]
                     (let [cell-index (.getCellIndex ^Trunk trunk)]
                       (.size cell-index) => 0))))
           (fact "Recover from backups"
                 (recover-backup) => anything)
           (fact "New cells in comming"
                 (let [trunks (.getTrunks ts/trunks)]
                   (doseq [trunk trunks]
                     (let [cell-index (.getCellIndex ^Trunk trunk)]
                       (doseq [[hash meta] cell-index]
                         (cell/read-cell trunk hash) => (contains {:data anything}))
                       (.size cell-index) => #(>= % 1)))))
           (fact "Check Recover"
                 (dorun
                   (map
                     (fn [id]
                       (let [str-key (str "test" id)]
                         (read-cell str-key) => (contains {:data {:str (str placr-holder id)}
                                                           :*id* (to-id str-key)})))
                     (range 300)))))
    (finally
      (fact "Clear Zookeeper Server"
            (clear-zk) => anything)
      (fact "Stop Server"
            (stop-server)  => anything)
      ;(FileUtils/deleteDirectory (File. "data"))
      (.stopZookeeper zk))))
